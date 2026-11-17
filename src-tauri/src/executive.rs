//! The Script Executive's commands: a script outlined without a server,
//! and run on a connection of its own, so a long script neither queues
//! behind the editors nor they behind it.
//!
//! A run starts on the registration the panel chose, or on nothing until
//! the script's own `CONNECT`. A `CONNECT` finds its password where it can,
//! in this order: the `PASSWORD` it wrote; the registration whose server,
//! database and user it names (its open session, then the keychain); and
//! last the user, asked through [`PASSWORD`] while the run waits. A
//! password is never logged, echoed in a message or kept in history.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use fb_core::{
    run_executive, split_database, ConnectParams, Connection, Connector, RunOptions, RunOutcome, RunStep, RunSummary,
    Target, TxEnd, TxOptions,
};
use fb_sql::isql::Connect;
use fb_sql::{StatementKind, Subject};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::commands::AppState;
use crate::error::{AppError, AppResult};
use crate::registry::{Engine, Registration};
use crate::script::Utf16;
use crate::secrets;

/// The event a run reports its statements through
pub const PROGRESS: &str = "executive-progress";
/// The event a run asks for a password through, waiting for
/// [`executive_password`]
pub const PASSWORD: &str = "executive-password";

/// Runs by the id the panel gave each: running, or over with a transaction
/// left active for the panel to end
#[derive(Default)]
pub struct Executives(Mutex<HashMap<u64, Arc<Run>>>);

impl Executives {
    fn get(&self, run: u64) -> Option<Arc<Run>> {
        self.0.lock().unwrap().get(&run).cloned()
    }
}

#[derive(Default)]
struct Run {
    /// Until its statements are through: only then can it be ended
    running: AtomicBool,
    cancel: AtomicBool,
    target: Mutex<Target>,
    /// Where the answer to a [`PASSWORD`] question goes, while one is asked
    answer: Mutex<Option<Sender<Option<String>>>>,
}

impl Run {
    fn target(&self) -> std::sync::MutexGuard<'_, Target> {
        self.target.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// One statement of a script, as the Statements tab and the explorer list
/// it
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Block {
    /// Among the script's statements, from 0, as a run numbers them
    index: usize,
    /// In the script's text, in UTF-16 code units as the editor counts
    from: usize,
    to: usize,
    /// Where it starts, from 1, and how many lines it spans
    line: usize,
    lines: usize,
    /// `sql` or `isql`
    kind: &'static str,
    subject: Subject,
    /// Its first characters, on one line
    summary: String,
}

/// Counts lines up to byte offsets that only grow
struct Lines<'a> {
    text: &'a str,
    byte: usize,
    line: usize,
}

impl<'a> Lines<'a> {
    fn new(text: &'a str) -> Self {
        Self { text, byte: 0, line: 1 }
    }

    fn at(&mut self, byte: usize) -> usize {
        let byte = byte.min(self.text.len());
        if byte < self.byte {
            (self.byte, self.line) = (0, 1);
        }
        self.line += self.text.as_bytes()[self.byte..byte].iter().filter(|&&b| b == b'\n').count();
        self.byte = byte;
        self.line
    }
}

const SUMMARY_CHARS: usize = 120;

fn summary(sql: &str) -> String {
    let mut out = String::new();
    for (i, word) in sql.split_whitespace().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        out.push_str(word);
        if out.chars().count() >= SUMMARY_CHARS {
            return out.chars().take(SUMMARY_CHARS).collect::<String>() + "…";
        }
    }
    out
}

/// The statements of `text`, each with what it is about
pub fn outline(text: &str) -> Vec<Block> {
    let mut offsets = Utf16::new(text);
    let mut lines = Lines::new(text);
    fb_sql::split(text)
        .into_iter()
        .filter(|s| !matches!(s.kind, StatementKind::SetTerm { .. }))
        .enumerate()
        .map(|(index, s)| {
            let sql = &text[s.range.clone()];
            let line = lines.at(s.range.start);
            let last = lines.at(s.range.end);
            Block {
                index,
                from: offsets.at(s.range.start),
                to: offsets.at(s.range.end),
                line,
                lines: last - line + 1,
                kind: if s.kind == StatementKind::Isql { "isql" } else { "sql" },
                subject: fb_sql::classify(sql),
                summary: summary(sql),
            }
        })
        .collect()
}

#[tauri::command]
pub async fn executive_outline(text: String) -> Vec<Block> {
    tauri::async_runtime::spawn_blocking(move || outline(&text)).await.unwrap_or_default()
}

/// What to do with a transaction still active when a script ends
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AtEnd {
    /// Leave it to the panel, which asks
    Ask,
    Commit,
    Rollback,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutiveOptions {
    stop_on_error: bool,
    auto_ddl: bool,
    rollback_on_stop: bool,
    at_end: AtEnd,
}

impl ExecutiveOptions {
    fn run_options(self) -> RunOptions {
        RunOptions {
            stop_on_error: self.stop_on_error,
            auto_ddl: self.auto_ddl,
            rollback_on_stop: self.rollback_on_stop,
            // A CONNECT cannot stop to ask: what is left behind is committed,
            // unless the panel said to roll back
            on_switch: if self.at_end == AtEnd::Rollback { TxEnd::Rollback } else { TxEnd::Commit },
            tx_options: TxOptions::default(),
        }
    }
}

/// A statement once run, for the panel
#[derive(Debug, Clone, Serialize)]
pub struct StepOut {
    index: usize,
    ms: f64,
    #[serde(flatten)]
    outcome: RunOutcome,
}

impl From<&RunStep> for StepOut {
    fn from(step: &RunStep) -> Self {
        Self { index: step.index, ms: step.ms, outcome: step.outcome.clone() }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    run: u64,
    steps: Vec<StepOut>,
    /// Statements gone through so far, of `total`
    at: usize,
    total: usize,
}

/// How a run ended, with the steps since the last [`PROGRESS`] event: the
/// answer and the events travel apart, and only the answer is sure to be
/// the last thing the panel hears
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunEnd {
    #[serde(flatten)]
    summary: RunSummary,
    steps: Vec<StepOut>,
    /// How the transaction left active was ended, as the options said
    ended: Option<&'static str>,
    /// Why ending it as they said failed: it was rolled back instead
    end_error: Option<String>,
}

/// The registration a `CONNECT` names: the same server, port, database and
/// user. Host and user compare without case, as do Windows paths.
fn matching<'a>(registrations: &'a [Registration], to: &Connect) -> Option<&'a Registration> {
    let (host, port, database) = split_database(&to.database);
    let host = host.unwrap_or_else(|| "localhost".into());
    let port = port.unwrap_or(ConnectParams::default().port);
    let local = |h: &str| matches!(h.to_ascii_lowercase().as_str(), "localhost" | "127.0.0.1" | "::1");
    registrations.iter().find(|r| {
        let Engine::Firebird(s) = &r.engine;
        let same_host = s.host.trim().eq_ignore_ascii_case(&host) || (local(s.host.trim()) && local(&host));
        same_host
            && s.port == port
            && s.database.trim().eq_ignore_ascii_case(database.trim())
            && to.user.as_deref().is_none_or(|u| s.user.trim().eq_ignore_ascii_case(u))
    })
}

/// A `CONNECT`'s password, if one is known without asking: the one it
/// wrote, else its registration's open session's, else its registration's
/// in the keychain
fn known_password(
    to: &Connect,
    registration: Option<&Registration>,
    connected: impl Fn(&str) -> Option<String>,
    saved: impl Fn(&str) -> Option<String>,
) -> Option<String> {
    if let Some(password) = &to.password {
        return Some(password.clone());
    }
    let registration = registration?;
    connected(&registration.id).or_else(|| registration.save_password.then(|| saved(&registration.id)).flatten())
}

/// What the panel is asked for a password with
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Ask {
    run: u64,
    /// The registration's name when the script's `CONNECT` matched one,
    /// else what it named
    database: String,
    user: String,
}

/// Opens what a script's `CONNECT` names
struct AppConnector {
    app: AppHandle,
    id: u64,
    run: Arc<Run>,
    /// The panel's execute charset, for a `CONNECT` with no `SET NAMES`
    charset: Option<String>,
}

impl AppConnector {
    /// Asks the panel, and waits for the answer or for the run to stop
    fn ask(&self, ask: Ask) -> Option<String> {
        let (tx, rx): (Sender<Option<String>>, Receiver<Option<String>>) = channel();
        *self.run.answer.lock().unwrap() = Some(tx);
        let _ = self.app.emit(PASSWORD, &ask);
        let answer = loop {
            match rx.recv_timeout(Duration::from_millis(200)) {
                Ok(answer) => break answer,
                Err(RecvTimeoutError::Timeout) if !self.run.cancel.load(Ordering::Relaxed) => {}
                Err(_) => break None,
            }
        };
        *self.run.answer.lock().unwrap() = None;
        answer
    }
}

impl Connector for AppConnector {
    fn connect(&mut self, to: &Connect, charset: Option<&str>) -> fb_core::Result<Connection> {
        let state = self.app.state::<AppState>();
        let registrations = state.registry.lock().unwrap().all().to_vec();
        let registration = matching(&registrations, to);
        let base = match registration {
            Some(r) => {
                let Engine::Firebird(settings) = &r.engine;
                settings.connect_params(String::new())
            }
            None => ConnectParams::default(),
        };
        let mut params = base.with_connect(to, charset.or(self.charset.as_deref()));
        let password = known_password(
            to,
            registration,
            |id| state.params(id).ok().map(|p| p.password),
            |id| secrets::get(id).ok().flatten(),
        );
        params.password = match password {
            Some(password) => password,
            None => {
                let ask = Ask {
                    run: self.id,
                    database: registration.map_or_else(|| to.database.clone(), |r| r.name.clone()),
                    user: params.user.clone(),
                };
                self.ask(ask).ok_or_else(|| fb_core::Error::Client { message: "No password was given.".into() })?
            }
        };
        Connection::open(&params)
    }
}

/// Where a run starts: the registration's parameters, the password found as
/// a `CONNECT` would find it
fn starting_params(state: &AppState, registration: &str, connector: &AppConnector) -> AppResult<ConnectParams> {
    if let Ok(params) = state.params(registration) {
        return Ok(params);
    }
    let r = state.registry.lock().unwrap().get(registration)?.clone();
    let Engine::Firebird(settings) = &r.engine;
    let password = match r.save_password.then(|| secrets::get(&r.id)).transpose()?.flatten() {
        Some(password) => password,
        None => connector
            .ask(Ask { run: connector.id, database: r.name.clone(), user: settings.user.clone() })
            .ok_or_else(|| AppError::PasswordRequired { registration: r.name.clone() })?,
    };
    Ok(settings.connect_params(password))
}

/// Runs `text`, or the statements of it numbered in `only`, on a connection
/// of the run's own: to `registration` when the panel chose one, else to
/// what the script's `CONNECT` names. `charset` is the panel's execute
/// charset, when it chose one. Every statement is reported through
/// [`PROGRESS`]. A transaction left active stays for [`executive_end`]
/// when the options say to ask.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn executive_run(
    app: AppHandle,
    run: u64,
    registration: Option<String>,
    charset: Option<String>,
    text: String,
    only: Option<Vec<usize>>,
    options: ExecutiveOptions,
) -> AppResult<RunEnd> {
    let state = app.state::<AppState>();
    let this = Arc::new(Run { running: AtomicBool::new(true), ..Run::default() });
    {
        let mut runs = state.executives.0.lock().unwrap();
        if runs.contains_key(&run) {
            return Err(AppError::Invalid { message: "This script is still running or has a transaction to end.".into() });
        }
        runs.insert(run, this.clone());
    }
    let events = app.clone();
    let worker = this.clone();
    let result = tauri::async_runtime::spawn_blocking(move || -> AppResult<RunEnd> {
        let state = events.state::<AppState>();
        let mut connector = AppConnector { app: events.clone(), id: run, run: worker.clone(), charset: charset.clone() };
        if let Some(registration) = &registration {
            let mut params = starting_params(&state, registration, &connector)?;
            if let Some(charset) = &charset {
                params.charset = charset.clone();
            }
            worker.target().conn = Some(Arc::new(Connection::open(&params)?));
        }

        let mut offsets = Progress { run, steps: Vec::new(), at: 0, total: 0 };
        let mut sent = Instant::now();
        let summary = run_executive(
            &worker.target,
            &text,
            only.as_deref(),
            options.run_options(),
            &mut connector,
            &worker.cancel,
            |step, total| {
                offsets.at += 1;
                offsets.total = total;
                offsets.steps.push(step.into());
                // Often enough to watch, seldom enough not to be the work
                if sent.elapsed() >= Duration::from_millis(100) || offsets.steps.len() >= 200 {
                    let _ = events.emit(PROGRESS, &offsets);
                    offsets.steps.clear();
                    sent = Instant::now();
                }
            },
        )?;

        let mut end = RunEnd { summary, steps: offsets.steps, ended: None, end_error: None };
        let how = match options.at_end {
            _ if !end.summary.active => None,
            AtEnd::Ask => None,
            AtEnd::Commit => Some(TxEnd::Commit),
            AtEnd::Rollback => Some(TxEnd::Rollback),
        };
        if let Some(how) = how {
            let mut target = worker.target();
            match target.end(how) {
                Ok(()) => end.ended = Some(if how == TxEnd::Commit { "commit" } else { "rollback" }),
                Err(e) => {
                    end.end_error = Some(e.to_string());
                    let _ = target.end(TxEnd::Rollback);
                    end.ended = Some("rollback");
                }
            }
            end.summary.active = false;
        }
        Ok(end)
    })
    .await
    .map_err(|e| AppError::Connection { message: format!("The script's worker failed: {e}") })
    .and_then(|r| r);
    this.running.store(false, Ordering::Relaxed);

    // Kept only with a transaction for the panel to end
    let keep = result.as_ref().is_ok_and(|end| end.summary.active);
    if !keep {
        state.executives.0.lock().unwrap().remove(&run);
    }
    result
}

/// Stops a run: before its next statement, the one running too when the
/// server can cancel it, and a password question unanswered. `false` when
/// no such run is going.
#[tauri::command]
pub fn executive_cancel(state: State<'_, AppState>, run: u64) -> bool {
    let Some(this) = state.executives.get(run) else { return false };
    this.cancel.store(true, Ordering::Relaxed);
    if let Some(answer) = this.answer.lock().unwrap().take() {
        let _ = answer.send(None);
    }
    // Not under the lock: the run takes it between statements
    let conn = this.target().conn.clone();
    if let Some(conn) = conn {
        // A server too old to cancel still stops between statements
        let _ = conn.cancel();
    }
    true
}

/// The answer to a [`PASSWORD`] question: `None` when the user would not
/// give one
#[tauri::command]
pub fn executive_password(state: State<'_, AppState>, run: u64, password: Option<String>) {
    if let Some(this) = state.executives.get(run) {
        if let Some(answer) = this.answer.lock().unwrap().take() {
            let _ = answer.send(password);
        }
    }
}

/// Ends the transaction a run left active, `commit` or `rollback`, and
/// closes its connection. A commit the server refuses is rolled back, and
/// its error returned.
#[tauri::command]
pub async fn executive_end(app: AppHandle, run: u64, how: String) -> AppResult<()> {
    let state = app.state::<AppState>();
    let Some(this) = state.executives.get(run) else { return Ok(()) };
    if this.running.load(Ordering::Relaxed) {
        return Err(AppError::Invalid { message: "The script is still running.".into() });
    }
    let how = if how == "commit" { TxEnd::Commit } else { TxEnd::Rollback };
    let ended = tauri::async_runtime::spawn_blocking(move || {
        let mut target = this.target();
        let ended = target.end(how);
        if ended.is_err() {
            let _ = target.end(TxEnd::Rollback);
        }
        target.conn = None;
        ended
    })
    .await
    .map_err(|e| AppError::Connection { message: format!("The script's worker failed: {e}") })?;
    state.executives.0.lock().unwrap().remove(&run);
    Ok(ended?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::{Auth, FirebirdSettings};

    #[test]
    fn an_outline_numbers_and_places_each_statement() {
        let text = "-- header\nSET TERM ^ ;\nALTER PROCEDURE P_AÇÃO AS\nBEGIN\nEND\n^\nSET TERM ; ^\n\
                    ------------------\nCOMMENT ON COLUMN PARAMETRO.COLUNA IS 'x';\nSET AUTODDL OFF;";
        let blocks = outline(text);
        assert_eq!(blocks.len(), 3);
        let b = &blocks[0];
        assert_eq!((b.index, b.line, b.lines, b.kind), (0, 3, 3, "sql"));
        assert_eq!(b.subject.name.as_deref(), Some("P_AÇÃO"));
        assert_eq!(b.summary, "ALTER PROCEDURE P_AÇÃO AS BEGIN END");
        let start = text.find("ALTER").unwrap();
        assert_eq!(b.from, text[..start].encode_utf16().count());
        assert_eq!((blocks[1].index, blocks[1].line, blocks[1].subject.member.as_deref()), (1, 9, Some("COLUNA")));
        assert_eq!((blocks[2].line, blocks[2].kind), (10, "isql"));
    }

    #[test]
    fn a_long_statement_is_summed_up_on_one_line() {
        let s = summary(&"x\n  ".repeat(200));
        assert_eq!(s.chars().count(), SUMMARY_CHARS + 1);
        assert!(s.ends_with('…') && !s.contains('\n'));
    }

    fn registration(id: &str, host: &str, port: u16, database: &str, save_password: bool) -> Registration {
        Registration {
            id: id.into(),
            name: format!("name of {id}"),
            folder: vec![],
            save_password,
            engine: Engine::Firebird(FirebirdSettings {
                host: host.into(),
                port,
                database: database.into(),
                user: "SYSDBA".into(),
                role: String::new(),
                charset: "UTF8".into(),
                dialect: fb_core::Dialect::D3,
                auth: Auth::Srp,
            }),
        }
    }

    fn connect(database: &str, user: Option<&str>, password: Option<&str>) -> Connect {
        Connect {
            database: database.into(),
            user: user.map(Into::into),
            password: password.map(Into::into),
            role: None,
        }
    }

    #[test]
    fn a_connect_finds_its_registration() {
        let all = [
            registration("a", "srv", 3050, "C:\\data\\x.fdb", true),
            registration("b", "localhost", 3051, "/db/y.fdb", false),
        ];
        let id = |to: &Connect| matching(&all, to).map(|r| r.id.as_str());
        assert_eq!(id(&connect("SRV:c:\\DATA\\x.fdb", Some("sysdba"), None)), Some("a"));
        assert_eq!(id(&connect("srv/3050:C:\\data\\x.fdb", None, None)), Some("a"));
        assert_eq!(id(&connect("srv/3051:C:\\data\\x.fdb", None, None)), None);
        assert_eq!(id(&connect("srv:C:\\data\\x.fdb", Some("OTHER"), None)), None);
        assert_eq!(id(&connect("127.0.0.1/3051:/db/y.fdb", None, None)), Some("b"));
    }

    #[test]
    fn a_password_is_looked_for_in_order() {
        let saved = registration("a", "srv", 3050, "db", true);
        let unsaved = registration("b", "srv", 3050, "db", false);
        let connected = |id: &str| (id == "open").then(|| "from the session".to_string());
        let keychain = |id: &str| Some(format!("{id} from the keychain"));
        let find = |to: &Connect, r: Option<&Registration>| known_password(to, r, connected, keychain);

        // The script's own comes first
        assert_eq!(find(&connect("db", None, Some("written")), Some(&saved)).as_deref(), Some("written"));
        // Then the registration's open session
        let open = Registration { id: "open".into(), ..unsaved.clone() };
        assert_eq!(find(&connect("db", None, None), Some(&open)).as_deref(), Some("from the session"));
        // Then the keychain, where the registration keeps one
        assert_eq!(find(&connect("db", None, None), Some(&saved)).as_deref(), Some("a from the keychain"));
        // Else the user is asked
        assert_eq!(find(&connect("db", None, None), Some(&unsaved)), None);
        assert_eq!(find(&connect("db", None, None), None), None);
    }
}
