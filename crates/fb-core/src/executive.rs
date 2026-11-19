//! The Script Executive's runner: a script as work in itself, run on a
//! connection of its own, all of it or only the statements chosen, every
//! statement's outcome reported as it ends.
//!
//! It differs from [`run_script`](crate::run_script), the editor's quick
//! run, in what it holds: the connection and the transaction live in a
//! [`Target`] the caller shares, so the caller can cancel the statement
//! running and, once the run is over, end what it left active.

use std::collections::HashSet;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

use fb_sql::isql::{self, Command};
use fb_sql::{ObjectKind, Operation, StatementKind};
use serde::{Deserialize, Serialize};

use crate::{
    ColumnInfo, Connection, CursorId, Error, ExecuteOptions, Records, Result, StatementType, TxEnd, TxId, TxOptions, Value,
};

/// What a statement fails with while the run has nowhere to send it
pub const NO_CONNECTION: &str = "No connection: choose a database or start the script with CONNECT";

/// Where a run sends its statements: the connection, if there is one, and
/// the transaction active on it
#[derive(Default)]
pub struct Target {
    pub conn: Option<Arc<Connection>>,
    pub tx: Option<TxId>,
}

impl Target {
    pub fn new(conn: Option<Connection>) -> Self {
        Self { conn: conn.map(Arc::new), tx: None }
    }

    /// Ends the active transaction, if any
    pub fn end(&mut self, how: TxEnd) -> Result<()> {
        let (Some(conn), Some(tx)) = (&self.conn, self.tx) else {
            self.tx = None;
            return Ok(());
        };
        let ended = conn.end(tx, how);
        if ended.is_ok() || how == TxEnd::Rollback {
            self.tx = None;
        }
        ended
    }
}

/// How deep `INPUT`s may go, a script reading one that reads another
pub const DEEPEST_INPUT: usize = 16;

/// Opens the connections a script's `CONNECT`s ask for, and reads the
/// scripts its `INPUT`s name
pub trait Connector {
    /// A connection to what `target` names, in `charset` when the script
    /// said one with `SET NAMES`
    fn connect(&mut self, target: &fb_sql::isql::Connect, charset: Option<&str>) -> Result<Connection>;

    /// The values of a statement's parameters, by position, `None` for a
    /// NULL: `names` has each position's `:name`, `None` for a `?`, and
    /// `types` what the server declares for it. `None` when they are not
    /// given, and the statement is not run.
    fn values(&mut self, sql: &str, names: &[Option<String>], types: &[ColumnInfo]) -> Option<Vec<Option<String>>> {
        let _ = (sql, names, types);
        None
    }

    /// Creates the database `create` names and connects to it, in
    /// `charset` when the script said one with `SET NAMES`
    fn create(&mut self, create: &fb_sql::isql::CreateDatabase, charset: Option<&str>) -> Result<Connection> {
        let _ = (create, charset);
        Err(Error::Client { message: "this run cannot create a database".into() })
    }

    /// The text of the script at `path`, which an `INPUT` names
    fn read(&mut self, path: &Path) -> Result<String> {
        std::fs::read_to_string(path).map_err(|e| Error::Client { message: e.to_string() })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunOptions {
    /// Stop at the first statement that fails, leaving the rest unrun
    pub stop_on_error: bool,
    /// Commit after each DDL statement, until the script says otherwise
    pub auto_ddl: bool,
    /// When a failure stops the run: roll back what was not committed
    pub rollback_on_stop: bool,
    /// What a `CONNECT` does with the transaction active on the connection
    /// it leaves
    pub on_switch: TxEnd,
    pub tx_options: TxOptions,
    /// Run what `SHELL` asks of the system's shell; without it, it is passed
    /// over and said so
    pub allow_shell: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum RunOutcome {
    Done {
        #[serde(rename = "statementType")]
        statement_type: StatementType,
        records: Records,
        /// A select's first rows, or what `SHOW` answers
        #[serde(skip_serializing_if = "Option::is_none")]
        rows: Option<Rows>,
    },
    /// An isql command followed here: `SET AUTODDL`, `SET NAMES`, `CONNECT`,
    /// `CREATE DATABASE`; with what of it was not, when something was not
    Applied {
        #[serde(skip_serializing_if = "Option::is_none")]
        note: Option<String>,
    },
    /// Not run, and why when there is more to say than "an isql command"
    Skipped { note: Option<String> },
    /// By the server, when running it or when committing it
    Failed { sqlcode: i32, message: String },
}

/// Rows a statement returned, as text: the first [`SHOWN`] of a select
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Rows {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
    /// It returned more than these
    pub more: bool,
}

/// How many rows of a select its step keeps
pub const SHOWN: usize = 50;

/// Where a statement run through an `INPUT` is
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Within {
    /// The file, as the run found it
    pub file: String,
    /// Its line there, from 1
    pub line: usize,
}

/// One statement of a script, once it ran
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RunStep {
    /// Among the script's statements, from 0; `SET TERM`s are not counted.
    /// For a statement of a file an `INPUT` reads, that `INPUT`'s.
    pub index: usize,
    /// In the text it is in, in bytes
    pub range: std::ops::Range<usize>,
    pub ms: f64,
    pub outcome: RunOutcome,
    /// Set for a statement of a file an `INPUT` reads. Of those only what
    /// fails, or is not run for a reason, is reported; the `INPUT` itself
    /// follows, failed when anything in its file did.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub within: Option<Within>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunSummary {
    /// How many statements were chosen to run
    pub statements: usize,
    pub done: usize,
    pub failed: usize,
    pub skipped: usize,
    /// DDL statements among those done: the catalog may have changed
    pub ddl: usize,
    /// Stopped by the caller before its end
    pub cancelled: bool,
    /// Stopped by a failure
    pub stopped: bool,
    /// A transaction is still active, for the caller to end
    pub active: bool,
    /// Statements run from the files `INPUT`s read, beside `statements`
    pub included: usize,
    pub elapsed_ms: f64,
}

/// The statements left out of the files the script's own `INPUT`s read, by
/// the `INPUT`'s index: a file not here is run whole
pub type Picks = std::collections::HashMap<usize, HashSet<usize>>;

/// Runs the statements of `text` numbered in `only` (all of them without
/// it), and in the file an `INPUT` of it reads all but those `within` leaves out on `target`, opening what a `CONNECT` asks for with `connector`,
/// and calling `on_step` after each with how many are chosen.
/// Setting `cancel` stops the run before its next statement; cancelling the
/// one running is [`Connection::cancel`]'s, on the target's connection.
///
/// An `INPUT` runs the file it names, all of it, where it stands, read by
/// `connector`; a relative name is found in `folder`, the script's, and in
/// a file read so, in that file's. `SET AUTODDL` and `SET NAMES` in it go on
/// holding after it, as in isql.
///
/// A statement the server refuses is a step that failed, not an error:
/// errors are for a connection that can run nothing more.
#[allow(clippy::too_many_arguments)]
pub fn run_executive(
    target: &Mutex<Target>,
    text: &str,
    folder: Option<&Path>,
    only: Option<&[usize]>,
    within: &Picks,
    options: RunOptions,
    connector: &mut dyn Connector,
    cancel: &AtomicBool,
    on_step: impl FnMut(&RunStep, usize),
) -> Result<RunSummary> {
    let started = Instant::now();
    let only: Option<HashSet<usize>> = only.map(|o| o.iter().copied().collect());
    // Read as they run rather than all at once: a file run from disk can be
    // hundreds of megabytes. Counted first, for the caller to say how far.
    let statements = || {
        fb_sql::Splitter::new(text)
            .filter(|s| !matches!(s.kind, StatementKind::SetTerm { .. }))
            .enumerate()
            .filter(|(i, _)| only.as_ref().is_none_or(|o| o.contains(i)))
    };
    let total = statements().count();
    let mut run = Runner {
        target,
        options,
        connector,
        cancel,
        on_step,
        auto_ddl: options.auto_ddl,
        names: None,
        summary: RunSummary {
            statements: total,
            done: 0,
            failed: 0,
            skipped: 0,
            ddl: 0,
            cancelled: false,
            stopped: false,
            active: false,
            included: 0,
            elapsed_ms: 0.0,
        },
        inputs: Vec::new(),
        top: 0,
        output: None,
        ended: false,
        within,
    };

    for (index, statement) in statements() {
        if cancel.load(Ordering::Relaxed) {
            run.summary.cancelled = true;
            break;
        }
        run.top = index;
        let at = Instant::now();
        let outcome = run.statement(&text[statement.range.clone()], statement.kind, folder)?;
        let summary = &mut run.summary;
        match &outcome {
            RunOutcome::Done { statement_type, .. } => {
                summary.done += 1;
                summary.ddl += usize::from(*statement_type == StatementType::Ddl);
            }
            RunOutcome::Applied { .. } => summary.done += 1,
            RunOutcome::Skipped { .. } => summary.skipped += 1,
            RunOutcome::Failed { .. } => summary.failed += 1,
        }
        let failed = matches!(outcome, RunOutcome::Failed { .. });
        let step = RunStep {
            index,
            range: statement.range.clone(),
            ms: at.elapsed().as_secs_f64() * 1000.0,
            outcome,
            within: None,
        };
        (run.on_step)(&step, total);
        if failed && run.options.stop_on_error {
            run.summary.stopped = true;
            if run.options.rollback_on_stop {
                run.lock().end(TxEnd::Rollback)?;
            }
            break;
        }
        // EXIT or QUIT
        if run.ended {
            break;
        }
    }
    let mut summary = run.summary.clone();
    // Cancelled while its last statement ran
    summary.cancelled |=
        cancel.load(Ordering::Relaxed) && summary.done + summary.failed + summary.skipped < summary.statements;
    summary.active = run.lock().tx.is_some();
    summary.elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
    Ok(summary)
}

/// What a run holds while it goes, through the files `INPUT`s read
struct Runner<'a, F: FnMut(&RunStep, usize)> {
    target: &'a Mutex<Target>,
    options: RunOptions,
    connector: &'a mut dyn Connector,
    cancel: &'a AtomicBool,
    on_step: F,
    auto_ddl: bool,
    /// SET NAMES: the character set of the next CONNECT
    names: Option<String>,
    summary: RunSummary,
    /// The files `INPUT`s are reading, outermost first
    inputs: Vec<PathBuf>,
    /// The statement of the script itself running, an `INPUT`'s whose file
    /// is being read
    top: usize,
    /// Where OUTPUT sends the rows of the selects
    output: Option<Output>,
    /// EXIT or QUIT was run: nothing more is
    ended: bool,
    /// What was left out of the files of the script's own `INPUT`s
    within: &'a Picks,
}

impl<F: FnMut(&RunStep, usize)> Runner<'_, F> {
    fn lock(&self) -> MutexGuard<'_, Target> {
        self.target.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn send(&mut self, sql: &str) -> Result<RunOutcome> {
        let sending = Sending {
            auto_ddl: self.auto_ddl,
            tx_options: self.options.tx_options,
            output: self.output.as_mut(),
            connector: &mut *self.connector,
        };
        send(self.target, sql, sending)
    }

    /// `OUTPUT file`: the rows of the selects that follow go to the file,
    /// written over, found as an `INPUT`'s is; `OUTPUT` alone, nowhere again
    fn output_to(&mut self, file: Option<&str>, folder: Option<&Path>) -> RunOutcome {
        if let Some(mut done) = self.output.take() {
            if let Err(e) = done.file.flush() {
                return RunOutcome::Failed { sqlcode: 0, message: format!("{}: {e}", done.path.display()) };
            }
        }
        let Some(name) = file else { return RunOutcome::Applied { note: None } };
        let named = Path::new(name);
        let path = match folder {
            _ if named.is_absolute() => named.to_path_buf(),
            Some(folder) => folder.join(named),
            None => {
                return RunOutcome::Failed {
                    sqlcode: 0,
                    message: format!("OUTPUT '{name}' names a file by a relative path, and the script is in no folder"),
                }
            }
        };
        match std::fs::File::create(&path) {
            Ok(file) => {
                let note = format!("The rows of the selects that follow go to {}", path.display());
                self.output = Some(Output { path, file: std::io::BufWriter::new(file) });
                RunOutcome::Applied { note: Some(note) }
            }
            Err(e) => RunOutcome::Failed { sqlcode: 0, message: format!("{}: {e}", path.display()) },
        }
    }

    /// `SHELL command`, in the script's folder, when the options allow it
    fn shell(&mut self, command: &str, folder: Option<&Path>) -> RunOutcome {
        if !self.options.allow_shell {
            return RunOutcome::Skipped {
                note: Some("SHELL is not run: the options say whether a script may start programs".into()),
            };
        }
        if command.is_empty() {
            return RunOutcome::Skipped { note: Some("SHELL with no command opens a shell: nothing to run".into()) };
        }
        let mut process = if cfg!(windows) {
            let mut p = std::process::Command::new("cmd");
            p.args(["/D", "/C", command]);
            p
        } else {
            let mut p = std::process::Command::new("sh");
            p.args(["-c", command]);
            p
        };
        if let Some(folder) = folder {
            process.current_dir(folder);
        }
        match process.stdin(std::process::Stdio::null()).output() {
            Ok(out) => {
                let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
                text.push_str(&String::from_utf8_lossy(&out.stderr));
                let mut text = text.trim().to_string();
                if text.len() > 4000 {
                    let mut cut = 4000;
                    while !text.is_char_boundary(cut) {
                        cut -= 1;
                    }
                    text.truncate(cut);
                    text.push_str("\n…");
                }
                if out.status.success() {
                    RunOutcome::Applied { note: Some(if text.is_empty() { "It ran, and wrote nothing".into() } else { text }) }
                } else {
                    let code = out.status.code().map_or_else(|| "a signal".to_string(), |c| c.to_string());
                    RunOutcome::Failed { sqlcode: 0, message: format!("It ended with {code}: {text}") }
                }
            }
            Err(e) => RunOutcome::Failed { sqlcode: 0, message: format!("The shell could not start: {e}") },
        }
    }

    /// Runs one statement, of the script or of a file it reads, whose
    /// relative `INPUT`s are found in `folder`
    fn statement(&mut self, sql: &str, kind: StatementKind, folder: Option<&Path>) -> Result<RunOutcome> {
        if kind == StatementKind::Isql {
            return Ok(match isql::command(sql) {
                Command::SetAutoDdl(on) => {
                    self.auto_ddl = on;
                    RunOutcome::Applied { note: None }
                }
                Command::SetNames(charset) => {
                    self.names = Some(charset);
                    RunOutcome::Applied { note: None }
                }
                Command::Connect(to) => {
                    let mut target = self.target.lock().unwrap_or_else(|e| e.into_inner());
                    switch(&mut target, &to, self.names.as_deref(), self.options.on_switch, &mut *self.connector)
                }
                Command::Input(name) => self.input(&name, folder)?,
                Command::Output(file) => self.output_to(file.as_deref(), folder),
                Command::Show(what) => match isql::show_query(&what) {
                    Some(query) => self.send(&query)?,
                    None => RunOutcome::Skipped {
                        note: Some(format!("SHOW {what} is not answered here: the explorer shows definitions")),
                    },
                },
                Command::Shell(command) => self.shell(&command, folder),
                Command::SetBail(on) => {
                    self.options.stop_on_error = on;
                    RunOutcome::Applied { note: None }
                }
                Command::Exit | Command::Quit => {
                    let how = if matches!(isql::command(sql), Command::Exit) { TxEnd::Commit } else { TxEnd::Rollback };
                    self.ended = true;
                    match self.lock().end(how) {
                        Ok(()) => RunOutcome::Applied { note: Some("The script ends here".into()) },
                        Err(e) => failure(e),
                    }
                }
                _ => match isql::describe_as_comment(sql) {
                    Some(comment) => self.send(&comment)?,
                    None => RunOutcome::Skipped { note: None },
                },
            });
        }
        let subject = fb_sql::classify(sql);
        if subject.kind == Some(ObjectKind::Database)
            && matches!(subject.operation, Operation::Create | Operation::Drop)
        {
            return Ok(match isql::command(sql) {
                Command::CreateDatabase(create) => {
                    let names = create.names.clone().or_else(|| self.names.clone());
                    let mut target = self.target.lock().unwrap_or_else(|e| e.into_inner());
                    created(&mut target, &create, names.as_deref(), self.options.on_switch, &mut *self.connector)
                }
                Command::DropDatabase => dropped(&mut self.target.lock().unwrap_or_else(|e| e.into_inner())),
                _ => RunOutcome::Failed { sqlcode: 0, message: "This CREATE or DROP DATABASE could not be read".into() },
            });
        }
        self.send(sql)
    }

    /// Runs the file an `INPUT` names, all of it: failed when anything in
    /// it failed, each failure reported with its file and line
    fn input(&mut self, name: &str, folder: Option<&Path>) -> Result<RunOutcome> {
        let failed = |message: String| Ok(RunOutcome::Failed { sqlcode: 0, message });
        let named = Path::new(name);
        let path = match folder {
            _ if named.is_absolute() => named.to_path_buf(),
            Some(folder) => folder.join(named),
            None => {
                return failed(format!(
                    "INPUT '{name}' names a file by a relative path, and the script is in no folder: \
                     save the script, or name the file in full"
                ))
            }
        };
        let same = path.canonicalize().unwrap_or_else(|_| path.clone());
        if self.inputs.contains(&same) {
            return failed(format!("INPUT '{name}' would read {} inside itself", path.display()));
        }
        if self.inputs.len() >= DEEPEST_INPUT {
            return failed(format!("INPUT '{name}': scripts read more than {DEEPEST_INPUT} deep"));
        }
        let text = match self.connector.read(&path) {
            Ok(text) => text,
            Err(e) => return failed(format!("INPUT '{name}': {} cannot be read: {e}", path.display())),
        };
        let file = path.display().to_string();
        let inner = path.parent().map(Path::to_path_buf);
        self.inputs.push(same);
        let (mut ran, mut failures) = (0, 0);
        // Lines counted as the statements come
        // Statements left out only of a file the script itself reads
        let left_out = if self.inputs.len() == 1 { self.within.get(&self.top) } else { None };
        let mut number = 0;
        let (mut line, mut counted) = (1, 0);
        for statement in fb_sql::Splitter::new(&text) {
            if matches!(statement.kind, StatementKind::SetTerm { .. }) {
                continue;
            }
            number += 1;
            if left_out.is_some_and(|out| out.contains(&(number - 1))) {
                continue;
            }
            if self.cancel.load(Ordering::Relaxed) {
                break;
            }
            let start = statement.range.start;
            line += text.as_bytes()[counted..start].iter().filter(|&&b| b == b'\n').count();
            counted = start;
            let at = Instant::now();
            let outcome = self.statement(&text[statement.range.clone()], statement.kind, inner.as_deref())?;
            ran += 1;
            self.summary.included += 1;
            if let RunOutcome::Done { statement_type: StatementType::Ddl, .. } = outcome {
                self.summary.ddl += 1;
            }
            let fails = matches!(outcome, RunOutcome::Failed { .. });
            failures += usize::from(fails);
            if fails || matches!(outcome, RunOutcome::Skipped { note: Some(_) }) {
                let step = RunStep {
                    index: self.top,
                    range: statement.range.clone(),
                    ms: at.elapsed().as_secs_f64() * 1000.0,
                    outcome,
                    within: Some(Within { file: file.clone(), line }),
                };
                (self.on_step)(&step, self.summary.statements);
            }
            if (fails && self.options.stop_on_error) || self.ended {
                break;
            }
        }
        self.inputs.pop();
        if failures == 0 {
            return Ok(RunOutcome::Applied { note: None });
        }
        failed(format!("{failures} of the {ran} statement(s) run from {file} failed"))
    }
}

/// What a server error is as a step
fn failure(e: Error) -> RunOutcome {
    match e {
        Error::Server { sqlcode, message } => RunOutcome::Failed { sqlcode, message },
        e => RunOutcome::Failed { sqlcode: 0, message: e.to_string() },
    }
}

/// Creates the database `create` names, leaving the target's connection as a
/// `CONNECT` does, and goes on on the new one, as isql does
fn created(
    target: &mut Target,
    create: &fb_sql::isql::CreateDatabase,
    charset: Option<&str>,
    how: TxEnd,
    connector: &mut dyn Connector,
) -> RunOutcome {
    let ended = target.end(how);
    target.conn = None;
    target.tx = None;
    if let Err(e) = ended {
        return failure(e);
    }
    match connector.create(create, charset) {
        Ok(conn) => {
            target.conn = Some(Arc::new(conn));
            let note = (!create.ignored.is_empty())
                .then(|| format!("Created without what it says of {}: set those by hand", create.ignored.join(", ")));
            RunOutcome::Applied { note }
        }
        Err(e) => failure(e),
    }
}

/// Deletes the database the target is connected to: what it left active is
/// rolled back, and the run goes on with no connection
fn dropped(target: &mut Target) -> RunOutcome {
    let Some(conn) = target.conn.clone() else {
        return RunOutcome::Failed { sqlcode: 0, message: NO_CONNECTION.into() };
    };
    let _ = target.end(TxEnd::Rollback);
    match conn.drop_database() {
        Ok(()) => {
            target.conn = None;
            RunOutcome::Applied { note: None }
        }
        Err(e) => failure(e),
    }
}

/// Leaves the target's connection, ending its transaction as `how` says,
/// for the one `to` names. A failure leaves no connection: what follows
/// must not run on the database the script meant to leave.
fn switch(
    target: &mut Target,
    to: &fb_sql::isql::Connect,
    charset: Option<&str>,
    how: TxEnd,
    connector: &mut dyn Connector,
) -> RunOutcome {
    let failed = |e: Error| match e {
        Error::Server { sqlcode, message } => RunOutcome::Failed { sqlcode, message },
        e => RunOutcome::Failed { sqlcode: 0, message: e.to_string() },
    };
    let ended = target.end(how);
    // Dropping it detaches, which rolls back a transaction a commit refused
    target.conn = None;
    target.tx = None;
    if let Err(e) = ended {
        return failed(e);
    }
    match connector.connect(to, charset) {
        Ok(conn) => {
            target.conn = Some(Arc::new(conn));
            RunOutcome::Applied { note: None }
        }
        Err(e) => failed(e),
    }
}

/// Where `OUTPUT` writes
struct Output {
    path: PathBuf,
    file: std::io::BufWriter<std::fs::File>,
}

/// A value as a script's output shows it
fn cell(value: &Value) -> String {
    match value {
        Value::Null => "<null>".into(),
        Value::Integer(n) => n.to_string(),
        Value::Decimal(d) => d.to_string(),
        Value::Float(f) => f.to_string(),
        Value::Text(s) => s.clone(),
        Value::Binary(b) => b.iter().map(|b| format!("{b:02X}")).collect(),
        Value::Boolean(b) => if *b { "TRUE" } else { "FALSE" }.into(),
        Value::Timestamp(t) => t.format("%Y-%m-%d %H:%M:%S%.3f").to_string(),
        Value::BlobRef { .. } => "<blob>".into(),
    }
}

/// Reads a select's rows to their end, keeping the first [`SHOWN`], and
/// writing every one to `output` when there is one: its column names, then
/// a line a row, tab-separated, and an empty line after
fn rows(conn: &Connection, cursor: CursorId, columns: &[ColumnInfo], mut output: Option<&mut Output>) -> Result<Rows> {
    let names: Vec<String> = columns.iter().map(|c| c.name.clone()).collect();
    let written = |out: &mut Output, line: String| {
        writeln!(out.file, "{line}").map_err(|e| Error::Client { message: format!("{}: {e}", out.path.display()) })
    };
    if let Some(out) = output.as_deref_mut() {
        written(out, names.join("\t"))?;
    }
    let mut shown = Rows { columns: names, rows: Vec::new(), more: false };
    loop {
        let fetched = conn.fetch(cursor, if output.is_some() { 500 } else { SHOWN + 1 })?;
        for row in fetched.rows {
            let row: Vec<String> = row.iter().map(cell).collect();
            if let Some(out) = output.as_deref_mut() {
                written(out, row.join("\t"))?;
            }
            if shown.rows.len() < SHOWN {
                shown.rows.push(row);
            } else {
                shown.more = true;
            }
        }
        if fetched.eof {
            break;
        }
        if output.is_none() && shown.more {
            conn.close_cursor(cursor)?;
            break;
        }
    }
    if let Some(out) = output {
        written(out, String::new())?;
    }
    Ok(shown)
}

/// Sends one statement to the target's connection, in its transaction or
/// one begun for it
/// What a statement is sent with, besides its target
struct Sending<'a> {
    auto_ddl: bool,
    tx_options: TxOptions,
    output: Option<&'a mut Output>,
    /// Asked for the values of its parameters
    connector: &'a mut dyn Connector,
}

fn send(target: &Mutex<Target>, sql: &str, sending: Sending) -> Result<RunOutcome> {
    let Sending { auto_ddl, tx_options, mut output, connector } = sending;
    let lock = || target.lock().unwrap_or_else(|e| e.into_inner());
    // Out of the lock while it runs: a cancel needs the connection
    let (conn, tx) = {
        let target = lock();
        let Some(conn) = target.conn.clone() else {
            return Ok(RunOutcome::Failed { sqlcode: 0, message: NO_CONNECTION.into() });
        };
        (conn, target.tx)
    };
    let tx = match tx {
        Some(tx) => tx,
        None => {
            let tx = conn.begin(tx_options)?;
            lock().tx = Some(tx);
            tx
        }
    };
    // No ? and no : is no parameter
    let bound = if sql.contains(['?', ':']) {
        fb_sql::parameters(sql)
    } else {
        fb_sql::Parameters { sql: sql.to_string(), names: Vec::new() }
    };
    let mut params = Vec::new();
    if !bound.names.is_empty() {
        let types = match conn.describe_params(tx, &bound.sql) {
            Ok(types) => types,
            Err(Error::Server { sqlcode, message }) => return Ok(RunOutcome::Failed { sqlcode, message }),
            Err(e) => return Err(e),
        };
        match connector.values(sql, &bound.names, &types) {
            Some(values) => params = values.into_iter().map(|v| v.map_or(Value::Null, Value::Text)).collect(),
            None => return Ok(RunOutcome::Skipped { note: Some("Not run: no parameter values were given".into()) }),
        }
    }
    let quiet = ExecuteOptions { plan: false, performance: false };
    let ran = conn.execute(tx, &bound.sql, params, quiet).and_then(|executed| {
        // The first rows are kept for the step; all of them go to OUTPUT's file
        let rows = match executed.cursor {
            Some(cursor) => Some(rows(&conn, cursor, &executed.columns, output.as_deref_mut())?),
            None => None,
        };
        if executed.ended.is_some_and(TxEnd::closes) {
            lock().tx = None;
        } else if auto_ddl && executed.statement_type == StatementType::Ddl {
            // Much of what is wrong with DDL shows only here
            lock().tx = None;
            conn.end(tx, TxEnd::Commit).or_else(|e| conn.end(tx, TxEnd::Rollback).and(Err(e)))?;
        }
        Ok(RunOutcome::Done { statement_type: executed.statement_type, records: executed.records, rows })
    });
    match ran {
        Ok(done) => Ok(done),
        Err(Error::Server { sqlcode, message }) => Ok(RunOutcome::Failed { sqlcode, message }),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(text: &str, only: Option<&[usize]>) -> (Vec<RunStep>, RunSummary) {
        run_with(text, only, false)
    }

    fn run_with(text: &str, only: Option<&[usize]>, allow_shell: bool) -> (Vec<RunStep>, RunSummary) {
        let target = Mutex::new(Target::new(None));
        let options = RunOptions {
            stop_on_error: false,
            auto_ddl: true,
            rollback_on_stop: true,
            on_switch: TxEnd::Commit,
            tx_options: TxOptions::default(),
            allow_shell,
        };
        let mut steps = Vec::new();
        let never = AtomicBool::new(false);
        let summary =
            run_executive(&target, text, None, only, &Picks::new(), options, &mut Refused, &never, |s, _| steps.push(s.clone()))
                .unwrap();
        (steps, summary)
    }

    /// Connects nowhere, saying with what it was asked
    struct Refused;

    impl Connector for Refused {
        fn connect(&mut self, target: &fb_sql::isql::Connect, charset: Option<&str>) -> Result<Connection> {
            Err(Error::Server { sqlcode: -902, message: format!("refused {} in {charset:?}", target.database) })
        }
    }

    #[test]
    fn shell_runs_only_when_allowed_and_bail_and_exit_hold() {
        let script = "SHELL echo hello;\nSET BAIL ON;\nselect 1 from rdb$database;\nselect 2 from rdb$database;";
        let (steps, summary) = run(script, None);
        let outcomes: Vec<&RunOutcome> = steps.iter().map(|s| &s.outcome).collect();
        assert!(matches!(outcomes[0], RunOutcome::Skipped { note: Some(n) } if n.contains("SHELL is not run")));
        // SET BAIL ON: the first failure stops the script
        assert_eq!((steps.len(), summary.stopped), (3, true), "{outcomes:?}");

        let (steps, _) = run_with("SHELL echo hello;\nSHELL exit 3;", None, true);
        assert!(matches!(&steps[0].outcome, RunOutcome::Applied { note: Some(n) } if n == "hello"), "{steps:?}");
        assert!(matches!(&steps[1].outcome, RunOutcome::Failed { message, .. } if message.starts_with("It ended with 3")));

        let (steps, summary) = run("SET AUTODDL ON;\nQUIT;\nselect 1 from rdb$database;", None);
        assert_eq!(steps.len(), 2);
        assert!(matches!(&steps[1].outcome, RunOutcome::Applied { note: Some(_) }));
        assert!(!summary.stopped);

        let (steps, _) = run("OUTPUT 'rows.txt';\nOUTPUT;\nSHOW GRANTS;", None);
        assert!(matches!(&steps[0].outcome, RunOutcome::Failed { message, .. } if message.contains("relative path")));
        assert!(matches!(&steps[1].outcome, RunOutcome::Applied { note: None }));
        assert!(matches!(&steps[2].outcome, RunOutcome::Skipped { note: Some(n) } if n.contains("SHOW GRANTS")));
    }

    #[test]
    fn isql_commands_and_what_is_not_run_without_a_server() {
        let script = "SET AUTODDL OFF;\nSET NAMES UTF8;\nCONNECT 'db' USER 'u';\nSET TERM ^ ;\nINPUT 'other.sql'^\nSET TERM ; ^\nCREATE DATABASE 'x.fdb';\nSET LIST ON;\nselect 1 from rdb$database;";
        let (steps, summary) = run(script, None);
        let outcomes: Vec<&RunOutcome> = steps.iter().map(|s| &s.outcome).collect();
        assert!(matches!(outcomes[0], RunOutcome::Applied { .. }), "{outcomes:?}");
        assert!(matches!(outcomes[1], RunOutcome::Applied { .. }), "{outcomes:?}");
        // The connector was asked in the character set SET NAMES gave
        assert!(matches!(outcomes[2], RunOutcome::Failed { message, .. } if message == "refused db in Some(\"UTF8\")"));
        // In no folder: a relative INPUT has nowhere to be found
        assert!(
            matches!(outcomes[3], RunOutcome::Failed { message, .. } if message.contains("relative path")),
            "{outcomes:?}"
        );
        // Created by the connector, which here cannot
        assert!(matches!(outcomes[4], RunOutcome::Failed { message, .. } if message.contains("cannot create")));
        assert!(matches!(outcomes[5], RunOutcome::Skipped { note: None }));
        assert!(matches!(outcomes[6], RunOutcome::Failed { message, .. } if message == NO_CONNECTION));
        assert_eq!(steps.iter().map(|s| s.index).collect::<Vec<_>>(), [0, 1, 2, 3, 4, 5, 6]);
        assert_eq!((summary.statements, summary.done, summary.skipped, summary.failed), (7, 2, 1, 4));
        assert!(!summary.active && !summary.stopped);
    }

    #[test]
    fn only_the_chosen_statements_run_or_are_reported() {
        let script = "select 1 from rdb$database; SET LIST ON; select 2 from rdb$database; SET COUNT ON;";
        let (steps, summary) = run(script, Some(&[1, 3, 9]));
        assert_eq!(steps.iter().map(|s| s.index).collect::<Vec<_>>(), [1, 3]);
        assert_eq!(&script[steps[1].range.clone()], "SET COUNT ON");
        assert_eq!((summary.statements, summary.skipped), (2, 2));
        let (steps, _) = run(script, Some(&[]));
        assert!(steps.is_empty());
    }

    /// Connects nowhere, and reads scripts from memory
    struct Files(Vec<(&'static str, &'static str)>);

    impl Connector for Files {
        fn connect(&mut self, target: &fb_sql::isql::Connect, charset: Option<&str>) -> Result<Connection> {
            Refused.connect(target, charset)
        }

        fn read(&mut self, path: &Path) -> Result<String> {
            let found = self.0.iter().find(|(p, _)| Path::new(p) == path);
            found.map(|(_, text)| text.to_string()).ok_or_else(|| Error::Client { message: "no such file".into() })
        }
    }

    fn run_in(text: &str, files: Files, stop_on_error: bool) -> (Vec<RunStep>, RunSummary) {
        run_left_out(text, files, stop_on_error, &Picks::new())
    }

    fn run_left_out(text: &str, files: Files, stop_on_error: bool, within: &Picks) -> (Vec<RunStep>, RunSummary) {
        let target = Mutex::new(Target::new(None));
        let options = RunOptions {
            stop_on_error,
            auto_ddl: true,
            rollback_on_stop: true,
            on_switch: TxEnd::Commit,
            tx_options: TxOptions::default(),
            allow_shell: false,
        };
        let mut steps = Vec::new();
        let never = AtomicBool::new(false);
        let mut files = files;
        let folder = Some(Path::new("/s"));
        let summary =
            run_executive(&target, text, folder, None, within, options, &mut files, &never, |s, _| steps.push(s.clone()))
                .unwrap();
        (steps, summary)
    }

    fn scripts() -> Files {
        Files(vec![
            ("/s/a.sql", "SET AUTODDL OFF;\nSET LIST ON;\n\nselect 1 from rdb$database;\nINPUT 'sub/b.sql';\n"),
            ("/s/sub/b.sql", "-- b\nSET TERM ^ ;\nCREATE DATABASE 'x.fdb'^\nSET TERM ; ^\n"),
            ("/s/loop.sql", "INPUT '/s/loop.sql';"),
        ])
    }

    #[test]
    fn an_input_runs_its_file_where_it_stands() {
        let script = "SET NAMES UTF8;\nINPUT 'a.sql';\nINPUT 'missing.sql';\nINPUT 'loop.sql';\nSET COUNT ON;";
        let (steps, summary) = run_in(script, scripts(), false);
        let seen: Vec<(usize, Option<(&str, usize)>, &RunOutcome)> =
            steps.iter().map(|s| (s.index, s.within.as_ref().map(|w| (w.file.as_str(), w.line)), &s.outcome)).collect();
        assert!(matches!(seen[0], (0, None, RunOutcome::Applied { .. })), "{seen:?}");
        // a.sql: its select has no connection, at its line 4; b.sql, read
        // from a.sql's folder, holds a CREATE DATABASE that fails, at its
        // line 3, and so a.sql's INPUT of it, at line 5
        assert!(
            matches!(seen[1], (1, Some(("/s/a.sql", 4)), RunOutcome::Failed { message, .. }) if message == NO_CONNECTION),
            "{seen:?}"
        );
        assert!(matches!(seen[2], (1, Some(("/s/sub/b.sql", 3)), RunOutcome::Failed { .. })), "{seen:?}");
        assert!(matches!(seen[3], (1, Some(("/s/a.sql", 5)), RunOutcome::Failed { .. })), "{seen:?}");
        assert!(
            matches!(seen[4], (1, None, RunOutcome::Failed { message, .. }) if message.contains("2 of the 4")),
            "{seen:?}"
        );
        assert!(
            matches!(seen[5], (2, None, RunOutcome::Failed { message, .. }) if message.contains("cannot be read")),
            "{seen:?}"
        );
        // A file that reads itself is stopped at the second time
        assert!(
            matches!(seen[6], (3, Some(("/s/loop.sql", 1)), RunOutcome::Failed { message, .. }) if message.contains("inside itself")),
            "{seen:?}"
        );
        assert!(matches!(seen[7], (3, None, RunOutcome::Failed { .. })), "{seen:?}");
        assert!(matches!(seen[8], (4, None, RunOutcome::Skipped { note: None })), "{seen:?}");
        assert_eq!((summary.statements, summary.done, summary.failed, summary.skipped), (5, 1, 3, 1));
        assert_eq!((summary.included, summary.stopped), (6, false));
    }

    #[test]
    fn what_is_left_out_of_an_input_is_not_run() {
        // a.sql's select (its 3rd) and its INPUT of b.sql (its 4th) left out:
        // nothing of it fails
        let within = Picks::from([(0, HashSet::from([2, 3]))]);
        let (steps, summary) = run_left_out("INPUT 'a.sql';", scripts(), false, &within);
        assert_eq!(steps.len(), 1, "{steps:?}");
        assert!(matches!(steps[0].outcome, RunOutcome::Applied { .. }), "{steps:?}");
        assert_eq!((summary.included, summary.failed), (2, 0));
    }

    #[test]
    fn a_failure_in_an_input_stops_the_script_when_it_stops_at_errors() {
        let (steps, summary) = run_in("INPUT 'a.sql';\nSET COUNT ON;", scripts(), true);
        let at: Vec<(usize, bool)> = steps.iter().map(|s| (s.index, s.within.is_some())).collect();
        // The select failed: b.sql is not read, nor what follows the INPUT
        assert_eq!(at, [(0, true), (0, false)]);
        assert_eq!((summary.failed, summary.included, summary.stopped), (1, 3, true));
    }
}
