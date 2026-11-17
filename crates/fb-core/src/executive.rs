//! The Script Executive's runner: a script as work in itself, run on a
//! connection of its own, all of it or only the statements chosen, every
//! statement's outcome reported as it ends.
//!
//! It differs from [`run_script`](crate::run_script), the editor's quick
//! run, in what it holds: the connection and the transaction live in a
//! [`Target`] the caller shares, so the caller can cancel the statement
//! running and, once the run is over, end what it left active.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
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

/// Opens the connections a script's `CONNECT`s ask for
pub trait Connector {
    /// A connection to what `target` names, in `charset` when the script
    /// said one with `SET NAMES`
    fn connect(&mut self, target: &fb_sql::isql::Connect, charset: Option<&str>) -> Result<Connection>;
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

/// One statement of a script, once it ran
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RunStep {
    /// Among the script's statements, from 0; `SET TERM`s are not counted
    pub index: usize,
    /// In the script's text, in bytes
    pub range: std::ops::Range<usize>,
    pub ms: f64,
    pub outcome: RunOutcome,
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
    pub elapsed_ms: f64,
}

/// Runs the statements of `text` numbered in `only` (all of them without
/// it) on `target`, opening what a `CONNECT` asks for with `connector`,
/// and calling `on_step` after each with how many are chosen.
/// Setting `cancel` stops the run before its next statement; cancelling the
/// one running is [`Connection::cancel`]'s, on the target's connection.
///
/// A statement the server refuses is a step that failed, not an error:
/// errors are for a connection that can run nothing more.
pub fn run_executive(
    target: &Mutex<Target>,
    text: &str,
    only: Option<&[usize]>,
    options: RunOptions,
    connector: &mut dyn Connector,
    cancel: &AtomicBool,
    mut on_step: impl FnMut(&RunStep, usize),
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
    let mut summary = RunSummary {
        statements: total,
        done: 0,
        failed: 0,
        skipped: 0,
        ddl: 0,
        cancelled: false,
        stopped: false,
        active: false,
        elapsed_ms: 0.0,
    };
    let mut auto_ddl = options.auto_ddl;
    // SET NAMES: the character set of the next CONNECT
    let mut names: Option<String> = None;
    let lock = || target.lock().unwrap_or_else(|e| e.into_inner());

    for (index, statement) in statements() {
        if cancel.load(Ordering::Relaxed) {
            summary.cancelled = true;
            break;
        }
        let sql = &text[statement.range.clone()];
        let at = Instant::now();
        let outcome = if statement.kind == StatementKind::Isql {
            match isql::command(sql) {
                Command::SetAutoDdl(on) => {
                    auto_ddl = on;
                    RunOutcome::Applied
                }
                Command::SetNames(charset) => {
                    names = Some(charset);
                    RunOutcome::Applied
                }
                Command::Connect(to) => switch(&mut lock(), &to, names.as_deref(), options.on_switch, connector),
                Command::Input(_) => {
                    RunOutcome::Skipped { note: Some("INPUT is not followed: run that script on its own".into()) }
                }
                _ => match isql::describe_as_comment(sql) {
                    Some(comment) => send(target, &comment, auto_ddl, options.tx_options)?,
                    None => RunOutcome::Skipped { note: None },
                },
            }
        } else {
            let subject = fb_sql::classify(sql);
            if subject.kind == Some(ObjectKind::Database)
                && matches!(subject.operation, Operation::Create | Operation::Drop)
            {
                RunOutcome::Skipped { note: Some("CREATE DATABASE and DROP DATABASE are not run from a script".into()) }
            } else {
                send(target, sql, auto_ddl, options.tx_options)?
            }
        };
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
        let step =
            RunStep { index, range: statement.range.clone(), ms: at.elapsed().as_secs_f64() * 1000.0, outcome };
        on_step(&step, summary.statements);
        if failed && options.stop_on_error {
            summary.stopped = true;
            if options.rollback_on_stop {
                lock().end(TxEnd::Rollback)?;
            }
            break;
        }
    }
    // Cancelled while its last statement ran
    summary.cancelled |=
        cancel.load(Ordering::Relaxed) && summary.done + summary.failed + summary.skipped < summary.statements;
    summary.active = lock().tx.is_some();
    summary.elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
    Ok(summary)
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
            run_executive(&target, text, only, options, &mut Refused, &never, |s, _| steps.push(s.clone())).unwrap();
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
        assert!(matches!(outcomes[3], RunOutcome::Skipped { note: Some(n) } if n.starts_with("INPUT")), "{outcomes:?}");
        assert!(matches!(outcomes[4], RunOutcome::Skipped { note: Some(n) } if n.contains("CREATE DATABASE")));
        assert!(matches!(outcomes[5], RunOutcome::Skipped { note: None }));
        assert!(matches!(outcomes[6], RunOutcome::Failed { message, .. } if message == NO_CONNECTION));
        assert_eq!(steps.iter().map(|s| s.index).collect::<Vec<_>>(), [0, 1, 2, 3, 4, 5, 6]);
        assert_eq!((summary.statements, summary.done, summary.skipped, summary.failed), (7, 2, 3, 2));
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
}
