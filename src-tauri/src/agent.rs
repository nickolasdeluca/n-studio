//! The agent: the user's own Claude Code, run in a terminal panel, with a
//! way into the connected databases that this app gives the shape of.
//!
//! The app does not talk to a model. It starts `claude` in a
//! pseudo-terminal, in a folder the user picked, signed in as the user
//! signed it in, and hands it an MCP server of its own (`mcp`) whose tools
//! (`tools`) read metadata and never data. A query the agent runs is shown
//! to the user, in an editor, and the agent hears only how it went; rows
//! reach the agent only when the user shares them (`agent_share`).
//!
//! One agent at a time: the panel's. What it was given (its token, the
//! rows shared with it) goes when it stops.

mod mcp;
mod tools;

use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::Mutex;
use std::time::Duration;

use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::error::{AppError, AppResult};

/// What the terminal printed: text, whole characters only
const OUTPUT: &str = "agent-output";
/// The process ended
const EXIT: &str = "agent-exit";
/// The agent asks the window for something: an editor to run a query in,
/// or to open SQL in
const REQUEST: &str = "agent-request";

/// The MCP server's name, which the agent's tools are called after
/// (`mcp__n-studio__run_query`)
const SERVER_NAME: &str = "n-studio";

/// The most rows, and the most characters in all, that can be shared at once
pub const SHARE_ROWS: usize = 1_000;
pub const SHARE_CHARS: usize = 400_000;

#[derive(Default)]
pub struct Agents {
    /// Which start the panel's process is, so what an older one says is
    /// told apart
    next: AtomicU64,
    running: Mutex<Option<Running>>,
    /// The MCP server: started with the first agent, kept with the app
    server: Mutex<Option<mcp::Server>>,
    /// The window's answers the agent's tools wait on, by request
    pending: Mutex<HashMap<u64, Sender<Value>>>,
    requests: AtomicU64,
}

struct Running {
    run: u64,
    /// What the agent's requests to the MCP server carry
    token: String,
    writer: Box<dyn Write + Send>,
    master: Box<dyn MasterPty + Send>,
    killer: Box<dyn ChildKiller + Send + Sync>,
    /// Written for this start: its MCP configuration and its instructions
    files: Vec<PathBuf>,
    shared: Vec<Shared>,
}

/// Rows the user chose to give the agent
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Shared {
    id: String,
    /// What the user called them, or where they are from
    label: String,
    /// The registration they were read from
    database: String,
    columns: Vec<SharedColumn>,
    rows: Vec<Vec<Option<String>>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SharedColumn {
    name: String,
    #[serde(rename = "type")]
    type_name: String,
}

#[derive(Clone, Serialize)]
struct Output {
    run: u64,
    data: String,
}

#[derive(Clone, Serialize)]
struct Exit {
    run: u64,
    /// None when it could not be told
    code: Option<u32>,
}

/// What the agent asks the window for
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Request {
    /// A select to run where the user sees it, read only
    Run { request: u64, registration: String, sql: String },
    /// SQL for the user to read, and to run if they want to
    Open { request: u64, registration: String, sql: String, title: Option<String> },
    /// A registered database to connect, as the explorer would
    Connect { request: u64, registration: String },
    /// A connected database to disconnect, unless the user has work open on it
    Disconnect { request: u64, registration: String },
}

impl Agents {
    /// Whether `token` is the running agent's
    fn admits(&self, token: &str) -> bool {
        self.running.lock().unwrap().as_ref().is_some_and(|r| r.token == token)
    }

    fn shared(&self) -> Vec<Shared> {
        self.running.lock().unwrap().as_ref().map(|r| r.shared.clone()).unwrap_or_default()
    }

    /// Asks the window for something and waits for its answer
    fn ask(&self, app: &AppHandle, make: impl FnOnce(u64) -> Request, wait: Duration) -> Result<Value, String> {
        let request = self.requests.fetch_add(1, Ordering::Relaxed) + 1;
        let (tx, rx) = mpsc::channel();
        self.pending.lock().unwrap().insert(request, tx);
        let sent = app.emit(REQUEST, make(request));
        let answer = match sent {
            Ok(()) => rx.recv_timeout(wait).map_err(|_| "N Studio did not answer in time.".to_string()),
            Err(e) => Err(format!("N Studio's window could not be reached: {e}")),
        };
        self.pending.lock().unwrap().remove(&request);
        answer
    }

    fn stop(&self) {
        if let Some(mut running) = self.running.lock().unwrap().take() {
            let _ = running.killer.kill();
            for file in &running.files {
                let _ = fs::remove_file(file);
            }
        }
    }
}

/// Decodes a stream of bytes into text, holding back a character split
/// between two reads
#[derive(Default)]
struct Utf8Stream {
    held: Vec<u8>,
}

impl Utf8Stream {
    fn push(&mut self, bytes: &[u8]) -> String {
        self.held.extend_from_slice(bytes);
        let mut text = String::new();
        loop {
            match std::str::from_utf8(&self.held) {
                Ok(all) => {
                    text.push_str(all);
                    self.held.clear();
                    return text;
                }
                Err(e) => {
                    let good = e.valid_up_to();
                    text.push_str(std::str::from_utf8(&self.held[..good]).unwrap());
                    match e.error_len() {
                        // Not UTF-8: shown as the replacement, and passed
                        Some(bad) => {
                            text.push('\u{FFFD}');
                            self.held.drain(..good + bad);
                        }
                        // A character the next read finishes
                        None => {
                            self.held.drain(..good);
                            return text;
                        }
                    }
                }
            }
        }
    }
}

/// What the agent is told about the app it runs in, after Claude Code's own
/// instructions
fn instructions() -> String {
    format!(
        "You are running inside N Studio, a Firebird database IDE, in a terminal panel the user opened. \
N Studio gives you the tools of its MCP server `{SERVER_NAME}`.\n\n\
- list_databases gives the databases registered in N Studio, connected or not. connect_database \
connects one (the user may be asked for its password, which you never see); disconnect_database \
disconnects one, and is refused while the user has work open on it.\n\
- You can read the metadata of connected databases: list_objects, get_object_ddl, get_dependencies, \
find_columns. Use them to help write queries and plan changes to tables, fields, procedures and \
triggers.\n\
- get_activity reads what a connected database is doing now (transaction counters, attachments, \
running statements, memory); get_trace_summary reads the Activity Monitor session the user records, \
ranked by cost. Use them to explain slowness, long transactions and garbage build-up. Statements come \
with their values replaced by ?.\n\
- You never see the data. run_query runs one SELECT in an editor the user sees, in a read-only \
transaction; the rows go to the user's grid and you are told only the columns, how many rows came, \
the times and the plan. Do not ask N Studio for rows another way.\n\
- To change a database, or to give the user a script, use open_in_editor: the user reads it and runs it \
themselves. Never try to write to a database by any other means.\n\
- The user may share rows with you from a grid; get_shared_data lists and reads them. Only what was \
shared is yours to analyse.\n\
- The SQL is Firebird's: mind the database's dialect, its server version and its character set, which \
list_databases gives."
    )
}

/// Where a start's files go: the app's cache, readable by the user only
fn start_dir(app: &AppHandle) -> AppResult<PathBuf> {
    let dir = app
        .path()
        .app_cache_dir()
        .map_err(|e| AppError::Storage { message: format!("The app's cache folder: {e}") })?
        .join("agent");
    fs::create_dir_all(&dir).map_err(|e| AppError::storage(&dir, e))?;
    Ok(dir)
}

fn write_private(path: &PathBuf, text: &str) -> AppResult<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path).map_err(|e| AppError::storage(path, e))?;
    file.write_all(text.as_bytes()).map_err(|e| AppError::storage(path, e))
}

/// The tools the agent runs without asking the user each time: they read
/// metadata, or what the user shared
fn allowed_tools() -> String {
    tools::READ_ONLY.iter().map(|t| format!("mcp__{SERVER_NAME}__{t}")).collect::<Vec<_>>().join(",")
}

/// `claude` with its arguments, started as the user's shell would find it
fn command(config: &PathBuf, prompt: &PathBuf) -> CommandBuilder {
    let args = [
        "--mcp-config".to_string(),
        config.display().to_string(),
        "--append-system-prompt-file".to_string(),
        prompt.display().to_string(),
        "--allowedTools".to_string(),
        allowed_tools(),
    ];
    #[cfg(unix)]
    {
        // A GUI app gets a short PATH; the user's login shell knows where
        // `claude` is. The arguments go through the environment, never
        // through the shell's quoting.
        let shell = std::env::var("SHELL")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| if cfg!(target_os = "macos") { "/bin/zsh" } else { "/bin/sh" }.to_string());
        let mut cmd = CommandBuilder::new(shell);
        let mut script = String::from("exec claude");
        for (i, arg) in args.iter().enumerate() {
            let var = format!("N_STUDIO_AGENT_ARG{i}");
            cmd.env(&var, arg);
            script.push_str(&format!(" \"${var}\""));
        }
        cmd.args(["-l", "-i", "-c", &script]);
        cmd
    }
    #[cfg(windows)]
    {
        let mut cmd = CommandBuilder::new(windows_claude());
        cmd.args(args);
        cmd
    }
}

/// `claude.exe` on the PATH, else where its installer puts it
#[cfg(windows)]
fn windows_claude() -> PathBuf {
    let on_path = std::env::var_os("PATH")
        .and_then(|path| std::env::split_paths(&path).map(|dir| dir.join("claude.exe")).find(|p| p.is_file()));
    on_path
        .or_else(|| {
            std::env::var_os("USERPROFILE")
                .map(|home| PathBuf::from(home).join(".local").join("bin").join("claude.exe"))
        })
        .unwrap_or_else(|| PathBuf::from("claude.exe"))
}

/// Where the agent works without a project: an empty folder of the app's
/// own, so it reaches none of the user's files and Claude Code asks to
/// trust it once
fn workspace(app: &AppHandle) -> AppResult<PathBuf> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| AppError::Storage { message: format!("The app's data folder: {e}") })?
        .join("agent")
        .join("workspace");
    fs::create_dir_all(&dir).map_err(|e| AppError::storage(&dir, e))?;
    Ok(dir)
}

/// Starts the agent in `folder`, or with no project in the app's own
/// workspace, in a terminal of `cols` by `rows`, ending one that runs.
/// Returns which start it is, which its output and its end are told with.
#[tauri::command]
pub async fn agent_start(app: AppHandle, folder: Option<String>, cols: u16, rows: u16) -> AppResult<u64> {
    tauri::async_runtime::spawn_blocking(move || start(&app, folder, cols, rows))
        .await
        .map_err(|e| AppError::Connection { message: format!("The worker failed: {e}") })?
}

fn start(app: &AppHandle, folder: Option<String>, cols: u16, rows: u16) -> AppResult<u64> {
    let agents = app.state::<Agents>();
    agents.stop();
    let folder = match folder.filter(|f| !f.is_empty()) {
        Some(folder) => PathBuf::from(folder),
        None => workspace(app)?,
    };
    if !folder.is_dir() {
        return Err(AppError::NotFound { message: format!("{} is not a folder.", folder.display()) });
    }

    let port = {
        let mut server = agents.server.lock().unwrap();
        if server.is_none() {
            *server = Some(mcp::Server::start(app.clone())?);
        }
        server.as_ref().unwrap().port()
    };
    let run = agents.next.fetch_add(1, Ordering::Relaxed) + 1;
    let token = format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple());

    let dir = start_dir(app)?;
    let config = dir.join(format!("mcp-{}.json", std::process::id()));
    let prompt = dir.join(format!("instructions-{}.md", std::process::id()));
    let mcp_config = serde_json::json!({
        "mcpServers": {
            SERVER_NAME: {
                "type": "http",
                "url": format!("http://127.0.0.1:{port}/mcp"),
                "headers": { "Authorization": format!("Bearer {token}") },
            }
        }
    });
    write_private(&config, &mcp_config.to_string())?;
    write_private(&prompt, &instructions())?;

    let pty = native_pty_system()
        .openpty(PtySize { rows: rows.max(2), cols: cols.max(10), pixel_width: 0, pixel_height: 0 })
        .map_err(|e| AppError::Connection { message: format!("No terminal could be opened: {e}") })?;
    let mut cmd = command(&config, &prompt);
    cmd.cwd(&folder);
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");
    let mut child = pty
        .slave
        .spawn_command(cmd)
        .map_err(|e| AppError::Connection { message: format!("Claude Code could not be started: {e}") })?;
    // The child has its end; ours reads when it is the only one left open
    drop(pty.slave);
    let killer = child.clone_killer();
    let mut reader = pty
        .master
        .try_clone_reader()
        .map_err(|e| AppError::Connection { message: format!("The terminal cannot be read: {e}") })?;
    let writer = pty
        .master
        .take_writer()
        .map_err(|e| AppError::Connection { message: format!("The terminal cannot be written to: {e}") })?;

    *agents.running.lock().unwrap() = Some(Running {
        run,
        token,
        writer,
        master: pty.master,
        killer,
        files: vec![config, prompt],
        shared: Vec::new(),
    });

    let events = app.clone();
    std::thread::Builder::new()
        .name(format!("agent-{run}"))
        .spawn(move || {
            let mut text = Utf8Stream::default();
            let mut buffer = [0u8; 16 * 1024];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let data = text.push(&buffer[..n]);
                        if !data.is_empty() {
                            let _ = events.emit(OUTPUT, Output { run, data });
                        }
                    }
                }
            }
            let code = child.wait().ok().map(|status| status.exit_code());
            let agents = events.state::<Agents>();
            let mut running = agents.running.lock().unwrap();
            if running.as_ref().is_some_and(|r| r.run == run) {
                if let Some(ended) = running.take() {
                    for file in &ended.files {
                        let _ = fs::remove_file(file);
                    }
                }
            }
            drop(running);
            let _ = events.emit(EXIT, Exit { run, code });
        })
        .map_err(|e| AppError::Connection { message: format!("The terminal cannot be read: {e}") })?;
    Ok(run)
}

/// The running agent, if it is the start `run`
fn with_running<T>(state: &Agents, run: u64, f: impl FnOnce(&mut Running) -> AppResult<T>) -> AppResult<T> {
    match state.running.lock().unwrap().as_mut() {
        Some(running) if running.run == run => f(running),
        _ => Err(AppError::NotFound { message: "The agent is not running.".into() }),
    }
}

/// What the user typed in the terminal
#[tauri::command]
pub fn agent_input(state: State<'_, Agents>, run: u64, data: String) -> AppResult<()> {
    with_running(&state, run, |r| {
        r.writer
            .write_all(data.as_bytes())
            .and_then(|()| r.writer.flush())
            .map_err(|e| AppError::Connection { message: format!("The terminal cannot be written to: {e}") })
    })
}

#[tauri::command]
pub fn agent_resize(state: State<'_, Agents>, run: u64, cols: u16, rows: u16) -> AppResult<()> {
    with_running(&state, run, |r| {
        r.master
            .resize(PtySize { rows: rows.max(2), cols: cols.max(10), pixel_width: 0, pixel_height: 0 })
            .map_err(|e| AppError::Connection { message: format!("The terminal cannot be resized: {e}") })
    })
}

#[tauri::command]
pub fn agent_stop(state: State<'_, Agents>) {
    state.stop();
}

/// The window's answer to an agent's request
#[tauri::command]
pub fn agent_reply(state: State<'_, Agents>, request: u64, reply: Value) {
    if let Some(waiting) = state.pending.lock().unwrap().remove(&request) {
        let _ = waiting.send(reply);
    }
}

/// Gives the running agent rows the user chose, and says the id it reads
/// them by
#[tauri::command]
pub fn agent_share(
    state: State<'_, Agents>,
    run: u64,
    label: String,
    database: String,
    columns: Vec<SharedColumn>,
    rows: Vec<Vec<Option<String>>>,
) -> AppResult<String> {
    check_share(&columns, &rows)?;
    with_running(&state, run, |r| {
        let id = format!("share-{}", r.shared.len() + 1);
        r.shared.push(Shared { id: id.clone(), label, database, columns, rows });
        Ok(id)
    })
}

fn check_share(columns: &[SharedColumn], rows: &[Vec<Option<String>>]) -> AppResult<()> {
    if columns.is_empty() || rows.is_empty() {
        return Err(AppError::Invalid { message: "There is nothing to share.".into() });
    }
    if rows.len() > SHARE_ROWS {
        return Err(AppError::Invalid { message: format!("At most {SHARE_ROWS} rows can be shared at once.") });
    }
    let chars: usize = rows.iter().flatten().map(|v| v.as_deref().map_or(4, str::len)).sum();
    if chars > SHARE_CHARS {
        return Err(AppError::Invalid {
            message: format!("At most {SHARE_CHARS} characters can be shared at once; share fewer rows or columns."),
        });
    }
    if rows.iter().any(|row| row.len() != columns.len()) {
        return Err(AppError::Invalid { message: "Every row must have a value for each column.".into() });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_character_split_between_reads_comes_whole() {
        let mut stream = Utf8Stream::default();
        let bytes = "ação".as_bytes();
        assert_eq!(stream.push(&bytes[..2]), "a");
        assert_eq!(stream.push(&bytes[2..4]), "ç");
        assert_eq!(stream.push(&bytes[4..]), "ão");
    }

    #[test]
    fn bytes_that_are_not_utf8_do_not_stop_the_stream() {
        let mut stream = Utf8Stream::default();
        assert_eq!(stream.push(b"a\xffb"), "a\u{FFFD}b");
    }

    #[test]
    fn a_share_too_large_or_ragged_is_refused() {
        let columns = vec![SharedColumn { name: "A".into(), type_name: "INTEGER".into() }];
        assert!(check_share(&columns, &[]).is_err());
        assert!(check_share(&columns, &[vec![Some("1".into()), None]]).is_err());
        let many = vec![vec![Some("1".to_string())]; SHARE_ROWS + 1];
        assert!(check_share(&columns, &many).is_err());
        assert!(check_share(&columns, &[vec![None]]).is_ok());
    }

    /// `claude` starts in a terminal as the panel starts it: found by the
    /// login shell, its arguments taken. Needs Claude Code installed:
    /// `cargo test -p n-studio -- --ignored claude_starts`
    #[test]
    #[ignore]
    fn claude_starts_in_a_terminal() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("mcp.json");
        let prompt = dir.path().join("instructions.md");
        fs::write(&config, r#"{"mcpServers":{}}"#).unwrap();
        fs::write(&prompt, instructions()).unwrap();
        let pty =
            native_pty_system().openpty(PtySize { rows: 30, cols: 100, pixel_width: 0, pixel_height: 0 }).unwrap();
        let mut cmd = command(&config, &prompt);
        cmd.cwd(dir.path());
        cmd.env("TERM", "xterm-256color");
        let mut child = pty.slave.spawn_command(cmd).unwrap();
        drop(pty.slave);
        let mut reader = pty.master.try_clone_reader().unwrap();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut stream = Utf8Stream::default();
            let mut buffer = [0u8; 4096];
            while let Ok(n) = reader.read(&mut buffer) {
                if n == 0 || tx.send(stream.push(&buffer[..n])).is_err() {
                    break;
                }
            }
        });
        let mut seen = String::new();
        let until = std::time::Instant::now() + Duration::from_secs(20);
        while std::time::Instant::now() < until {
            if let Ok(text) = rx.recv_timeout(Duration::from_millis(200)) {
                seen.push_str(&text);
            }
            if child.try_wait().unwrap().is_some() {
                break;
            }
        }
        let alive = child.try_wait().unwrap().is_none();
        let _ = child.kill();
        assert!(alive, "claude ended:\n{seen}");
        assert!(!seen.contains("command not found") && !seen.contains("unknown option"), "{seen}");
    }

    #[test]
    fn the_metadata_tools_are_the_ones_allowed_without_asking() {
        let allowed = allowed_tools();
        assert!(allowed.contains("mcp__n-studio__get_object_ddl"));
        assert!(!allowed.contains("run_query"));
        assert!(!allowed.contains("open_in_editor"));
        assert!(!allowed.contains("connect_database") && !allowed.contains("disconnect_database"));
    }
}
