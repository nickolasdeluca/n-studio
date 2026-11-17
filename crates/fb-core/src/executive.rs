//! The Script Executive's runner: a script as work in itself, run on a
//! connection of its own, all of it or only the statements chosen, every
//! statement's outcome reported as it ends.
//!
//! It differs from [`run_script`](crate::run_script), the editor's quick
//! run, in what it holds: the connection and the transaction live in a
//! [`Target`] the caller shares, so the caller can cancel the statement
//! running and, once the run is over, end what it left active.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

use fb_sql::isql::{self, Command};
use fb_sql::{ObjectKind, Operation, StatementKind};
use serde::{Deserialize, Serialize};

use crate::{Connection, Error, ExecuteOptions, Records, Result, StatementType, TxEnd, TxId, TxOptions};

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
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum RunOutcome {
    Done {
        #[serde(rename = "statementType")]
        statement_type: StatementType,
        records: Records,
    },
    /// An isql command followed here: `SET AUTODDL`, `SET NAMES`, `CONNECT`
    Applied,
    /// Not run, and why when there is more to say than "an isql command"
    Skipped { note: Option<String> },
    /// By the server, when running it or when committing it
    Failed { sqlcode: i32, message: String },
}

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

/// Runs the statements of `text` numbered in `only` (all of them without
/// it) on `target`, opening what a `CONNECT` asks for with `connector`,
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
            RunOutcome::Applied => summary.done += 1,
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
        if failed && options.stop_on_error {
            run.summary.stopped = true;
            if options.rollback_on_stop {
                run.lock().end(TxEnd::Rollback)?;
            }
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
}

impl<F: FnMut(&RunStep, usize)> Runner<'_, F> {
    fn lock(&self) -> MutexGuard<'_, Target> {
        self.target.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Runs one statement, of the script or of a file it reads, whose
    /// relative `INPUT`s are found in `folder`
    fn statement(&mut self, sql: &str, kind: StatementKind, folder: Option<&Path>) -> Result<RunOutcome> {
        if kind == StatementKind::Isql {
            return Ok(match isql::command(sql) {
                Command::SetAutoDdl(on) => {
                    self.auto_ddl = on;
                    RunOutcome::Applied
                }
                Command::SetNames(charset) => {
                    self.names = Some(charset);
                    RunOutcome::Applied
                }
                Command::Connect(to) => {
                    let mut target = self.target.lock().unwrap_or_else(|e| e.into_inner());
                    switch(&mut target, &to, self.names.as_deref(), self.options.on_switch, &mut *self.connector)
                }
                Command::Input(name) => self.input(&name, folder)?,
                _ => match isql::describe_as_comment(sql) {
                    Some(comment) => send(self.target, &comment, self.auto_ddl, self.options.tx_options)?,
                    None => RunOutcome::Skipped { note: None },
                },
            });
        }
        let subject = fb_sql::classify(sql);
        if subject.kind == Some(ObjectKind::Database)
            && matches!(subject.operation, Operation::Create | Operation::Drop)
        {
            return Ok(RunOutcome::Skipped {
                note: Some("CREATE DATABASE and DROP DATABASE are not run from a script".into()),
            });
        }
        send(self.target, sql, self.auto_ddl, self.options.tx_options)
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
        let (mut line, mut counted) = (1, 0);
        for statement in fb_sql::Splitter::new(&text) {
            if matches!(statement.kind, StatementKind::SetTerm { .. }) {
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
            if fails && self.options.stop_on_error {
                break;
            }
        }
        self.inputs.pop();
        if failures == 0 {
            return Ok(RunOutcome::Applied);
        }
        failed(format!("{failures} of the {ran} statement(s) run from {file} failed"))
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
            RunOutcome::Applied
        }
        Err(e) => failed(e),
    }
}

/// Sends one statement to the target's connection, in its transaction or
/// one begun for it
fn send(target: &Mutex<Target>, sql: &str, auto_ddl: bool, tx_options: TxOptions) -> Result<RunOutcome> {
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
    let quiet = ExecuteOptions { plan: false, performance: false };
    let ran = conn.execute(tx, sql, vec![], quiet).and_then(|executed| {
        // A script shows no rows: its selects only have to run
        if let Some(cursor) = executed.cursor {
            conn.close_cursor(cursor)?;
        }
        if executed.ended.is_some_and(TxEnd::closes) {
            lock().tx = None;
        } else if auto_ddl && executed.statement_type == StatementType::Ddl {
            // Much of what is wrong with DDL shows only here
            lock().tx = None;
            conn.end(tx, TxEnd::Commit).or_else(|e| conn.end(tx, TxEnd::Rollback).and(Err(e)))?;
        }
        Ok(RunOutcome::Done { statement_type: executed.statement_type, records: executed.records })
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
        let target = Mutex::new(Target::new(None));
        let options = RunOptions {
            stop_on_error: false,
            auto_ddl: true,
            rollback_on_stop: true,
            on_switch: TxEnd::Commit,
            tx_options: TxOptions::default(),
        };
        let mut steps = Vec::new();
        let never = AtomicBool::new(false);
        let summary =
            run_executive(&target, text, None, only, options, &mut Refused, &never, |s, _| steps.push(s.clone()))
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
    fn isql_commands_and_what_is_not_run_without_a_server() {
        let script = "SET AUTODDL OFF;\nSET NAMES UTF8;\nCONNECT 'db' USER 'u';\nSET TERM ^ ;\nINPUT 'other.sql'^\nSET TERM ; ^\nCREATE DATABASE 'x.fdb';\nSHOW TABLES;\nselect 1 from rdb$database;";
        let (steps, summary) = run(script, None);
        let outcomes: Vec<&RunOutcome> = steps.iter().map(|s| &s.outcome).collect();
        assert!(matches!(outcomes[0], RunOutcome::Applied), "{outcomes:?}");
        assert!(matches!(outcomes[1], RunOutcome::Applied), "{outcomes:?}");
        // The connector was asked in the character set SET NAMES gave
        assert!(matches!(outcomes[2], RunOutcome::Failed { message, .. } if message == "refused db in Some(\"UTF8\")"));
        // In no folder: a relative INPUT has nowhere to be found
        assert!(
            matches!(outcomes[3], RunOutcome::Failed { message, .. } if message.contains("relative path")),
            "{outcomes:?}"
        );
        assert!(matches!(outcomes[4], RunOutcome::Skipped { note: Some(n) } if n.contains("CREATE DATABASE")));
        assert!(matches!(outcomes[5], RunOutcome::Skipped { note: None }));
        assert!(matches!(outcomes[6], RunOutcome::Failed { message, .. } if message == NO_CONNECTION));
        assert_eq!(steps.iter().map(|s| s.index).collect::<Vec<_>>(), [0, 1, 2, 3, 4, 5, 6]);
        assert_eq!((summary.statements, summary.done, summary.skipped, summary.failed), (7, 2, 2, 3));
        assert!(!summary.active && !summary.stopped);
    }

    #[test]
    fn only_the_chosen_statements_run_or_are_reported() {
        let script = "select 1 from rdb$database; SHOW TABLES; select 2 from rdb$database; SHOW VIEWS;";
        let (steps, summary) = run(script, Some(&[1, 3, 9]));
        assert_eq!(steps.iter().map(|s| s.index).collect::<Vec<_>>(), [1, 3]);
        assert_eq!(&script[steps[1].range.clone()], "SHOW VIEWS");
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
        let target = Mutex::new(Target::new(None));
        let options = RunOptions {
            stop_on_error,
            auto_ddl: true,
            rollback_on_stop: true,
            on_switch: TxEnd::Commit,
            tx_options: TxOptions::default(),
        };
        let mut steps = Vec::new();
        let never = AtomicBool::new(false);
        let mut files = files;
        let folder = Some(Path::new("/s"));
        let summary =
            run_executive(&target, text, folder, None, options, &mut files, &never, |s, _| steps.push(s.clone()))
                .unwrap();
        (steps, summary)
    }

    fn scripts() -> Files {
        Files(vec![
            ("/s/a.sql", "SET AUTODDL OFF;\nSHOW TABLES;\n\nselect 1 from rdb$database;\nINPUT 'sub/b.sql';\n"),
            ("/s/sub/b.sql", "-- b\nSET TERM ^ ;\nCREATE DATABASE 'x.fdb'^\nSET TERM ; ^\n"),
            ("/s/loop.sql", "INPUT '/s/loop.sql';"),
        ])
    }

    #[test]
    fn an_input_runs_its_file_where_it_stands() {
        let script = "SET NAMES UTF8;\nINPUT 'a.sql';\nINPUT 'missing.sql';\nINPUT 'loop.sql';\nSHOW VIEWS;";
        let (steps, summary) = run_in(script, scripts(), false);
        let seen: Vec<(usize, Option<(&str, usize)>, &RunOutcome)> =
            steps.iter().map(|s| (s.index, s.within.as_ref().map(|w| (w.file.as_str(), w.line)), &s.outcome)).collect();
        assert!(matches!(seen[0], (0, None, RunOutcome::Applied)), "{seen:?}");
        // a.sql: its select has no connection, at its line 4; b.sql, read
        // from a.sql's folder, holds what is not run, at its line 3
        assert!(
            matches!(seen[1], (1, Some(("/s/a.sql", 4)), RunOutcome::Failed { message, .. }) if message == NO_CONNECTION),
            "{seen:?}"
        );
        assert!(matches!(seen[2], (1, Some(("/s/sub/b.sql", 3)), RunOutcome::Skipped { note: Some(_) })), "{seen:?}");
        assert!(
            matches!(seen[3], (1, None, RunOutcome::Failed { message, .. }) if message.contains("1 of the 4")),
            "{seen:?}"
        );
        assert!(
            matches!(seen[4], (2, None, RunOutcome::Failed { message, .. }) if message.contains("cannot be read")),
            "{seen:?}"
        );
        // A file that reads itself is stopped at the second time
        assert!(
            matches!(seen[5], (3, Some(("/s/loop.sql", 1)), RunOutcome::Failed { message, .. }) if message.contains("inside itself")),
            "{seen:?}"
        );
        assert!(matches!(seen[6], (3, None, RunOutcome::Failed { .. })), "{seen:?}");
        assert!(matches!(seen[7], (4, None, RunOutcome::Skipped { note: None })), "{seen:?}");
        assert_eq!((summary.statements, summary.done, summary.failed, summary.skipped), (5, 1, 3, 1));
        assert_eq!((summary.included, summary.stopped), (6, false));
    }

    #[test]
    fn a_failure_in_an_input_stops_the_script_when_it_stops_at_errors() {
        let (steps, summary) = run_in("INPUT 'a.sql';\nSHOW VIEWS;", scripts(), true);
        let at: Vec<(usize, bool)> = steps.iter().map(|s| (s.index, s.within.is_some())).collect();
        // The select failed: b.sql is not read, nor what follows the INPUT
        assert_eq!(at, [(0, true), (0, false)]);
        assert_eq!((summary.failed, summary.included, summary.stopped), (1, 3, true));
    }
}
