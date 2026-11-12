//! Scripts: many statements run one after the other, as isql would, each
//! one's outcome reported as it ends.
//!
//! A script holds its own `SET TERM`s, which the splitter follows, and may
//! commit or roll back as it goes. DDL is committed as soon as it runs
//! unless the script turns that off (`SET AUTODDL OFF`): the statements
//! after a `CREATE TABLE` usually need the table to be there. The other
//! isql commands mean nothing to a server and are passed over.

use std::ops::Range;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use fb_sql::StatementKind;
use serde::{Deserialize, Serialize};

use crate::{Connection, Error, ExecuteOptions, Records, Result, StatementType, TxEnd, TxId, TxOptions};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScriptOptions {
    /// Stop at the first statement that fails, leaving the rest unrun
    pub stop_on_error: bool,
    /// Commit after each DDL statement, until the script says otherwise
    pub auto_ddl: bool,
    /// All or nothing, in a transaction of the script's own: committed when
    /// every statement ran, rolled back otherwise. Stops at the first
    /// failure and commits nothing on the way.
    pub atomic: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum StepOutcome {
    Done {
        #[serde(rename = "statementType")]
        statement_type: StatementType,
        records: Records,
    },
    /// Not for the server: an isql command
    Skipped,
    /// By the server, when running it or when committing it
    Failed { sqlcode: i32, message: String },
}

/// One statement of a script, once it ran
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Step {
    /// Among the script's statements, from 0; `SET TERM`s are not counted
    pub index: usize,
    /// In the script's text, in bytes
    pub range: Range<usize>,
    pub ms: f64,
    pub outcome: StepOutcome,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScriptSummary {
    /// How many statements the script has, run or not
    pub statements: usize,
    pub done: usize,
    pub failed: usize,
    pub skipped: usize,
    /// DDL statements among those done: the catalog may have changed
    pub ddl: usize,
    /// Stopped by the caller before its end
    pub cancelled: bool,
    /// The transaction left active, for the caller to end
    pub tx: Option<TxId>,
    /// An atomic script's end: committed or rolled back
    pub ended: Option<TxEnd>,
    pub elapsed_ms: f64,
}

/// Runs `text`, statement by statement, calling `on_step` after each.
/// `tx` is the transaction to start in, if the caller has one; another is
/// begun with `tx_options` whenever one is needed and none is active.
/// Setting `cancel` stops the script before its next statement; cancelling
/// the one running is [`Connection::cancel`]'s.
///
/// A statement the server refuses is a step that failed, not an error:
/// errors are for a connection that can run nothing more.
pub fn run_script(
    conn: &Connection,
    tx: Option<TxId>,
    tx_options: TxOptions,
    text: &str,
    options: ScriptOptions,
    cancel: &AtomicBool,
    mut on_step: impl FnMut(&Step, usize),
) -> Result<ScriptSummary> {
    let started = Instant::now();
    let statements: Vec<_> =
        fb_sql::split(text).into_iter().filter(|s| !matches!(s.kind, StatementKind::SetTerm { .. })).collect();
    let mut summary = ScriptSummary {
        statements: statements.len(),
        done: 0,
        failed: 0,
        skipped: 0,
        ddl: 0,
        cancelled: false,
        tx: if options.atomic { None } else { tx },
        ended: None,
        elapsed_ms: 0.0,
    };
    let mut auto_ddl = options.auto_ddl && !options.atomic;
    let stop_on_error = options.stop_on_error || options.atomic;
    let quiet = ExecuteOptions { plan: false, performance: false };

    for (index, statement) in statements.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            summary.cancelled = true;
            break;
        }
        let sql = &text[statement.range.clone()];
        let at = Instant::now();
        let outcome = if statement.kind == StatementKind::Isql {
            if let Some(on) = set_auto_ddl(sql) {
                auto_ddl = on && !options.atomic;
            }
            StepOutcome::Skipped
        } else {
            let tx = match summary.tx {
                Some(tx) => tx,
                None => {
                    let tx = conn.begin(tx_options)?;
                    summary.tx = Some(tx);
                    tx
                }
            };
            let ran = conn.execute(tx, sql, vec![], quiet).and_then(|executed| {
                // A script shows no rows: its selects only have to run
                if let Some(cursor) = executed.cursor {
                    conn.close_cursor(cursor)?;
                }
                if executed.ended.is_some_and(TxEnd::closes) {
                    summary.tx = None;
                } else if auto_ddl && executed.statement_type == StatementType::Ddl {
                    // Much of what is wrong with DDL shows only here
                    summary.tx = None;
                    conn.end(tx, TxEnd::Commit).or_else(|e| conn.end(tx, TxEnd::Rollback).and(Err(e)))?;
                }
                Ok(StepOutcome::Done { statement_type: executed.statement_type, records: executed.records })
            });
            match ran {
                Ok(done) => done,
                Err(Error::Server { sqlcode, message }) => StepOutcome::Failed { sqlcode, message },
                Err(e) => return Err(e),
            }
        };
        match outcome {
            StepOutcome::Done { statement_type, .. } => {
                summary.done += 1;
                summary.ddl += usize::from(statement_type == StatementType::Ddl);
            }
            StepOutcome::Skipped => summary.skipped += 1,
            StepOutcome::Failed { .. } => summary.failed += 1,
        }
        let failed = matches!(outcome, StepOutcome::Failed { .. });
        let step = Step { index, range: statement.range.clone(), ms: at.elapsed().as_secs_f64() * 1000.0, outcome };
        on_step(&step, summary.statements);
        if failed && stop_on_error {
            break;
        }
    }
    // Cancelled while its last statement ran
    summary.cancelled |= cancel.load(Ordering::Relaxed) && summary.done + summary.failed + summary.skipped < summary.statements;

    if options.atomic {
        if let Some(tx) = summary.tx.take() {
            let whole = summary.failed == 0 && !summary.cancelled;
            let how = if whole { TxEnd::Commit } else { TxEnd::Rollback };
            match conn.end(tx, how) {
                Ok(()) => summary.ended = Some(how),
                Err(Error::Server { sqlcode, message }) if whole => {
                    // The commit is where the script failed, then
                    conn.end(tx, TxEnd::Rollback)?;
                    summary.ended = Some(TxEnd::Rollback);
                    summary.failed += 1;
                    summary.done = summary.done.saturating_sub(1);
                    let last = statements.last().map_or(0..0, |s| s.range.clone());
                    let outcome = StepOutcome::Failed { sqlcode, message };
                    on_step(&Step { index: statements.len().saturating_sub(1), range: last, ms: 0.0, outcome }, summary.statements);
                }
                Err(e) => return Err(e),
            }
        }
    }
    summary.elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
    Ok(summary)
}

/// What a `SET AUTODDL` or `SET AUTO` command sets it to; it toggles when
/// it does not say
fn set_auto_ddl(command: &str) -> Option<bool> {
    let mut words = command.split_whitespace().map(str::to_ascii_uppercase);
    if words.next().as_deref() != Some("SET") || !matches!(words.next().as_deref(), Some("AUTODDL" | "AUTO")) {
        return None;
    }
    match words.next().as_deref() {
        Some("OFF") => Some(false),
        // isql toggles without an argument; a script that bothers to say it
        // nearly always means to turn it on
        _ => Some(true),
    }
}

#[cfg(test)]
mod tests {
    use super::set_auto_ddl;

    #[test]
    fn autoddl_is_the_one_isql_command_followed() {
        assert_eq!(set_auto_ddl("SET AUTODDL OFF"), Some(false));
        assert_eq!(set_auto_ddl("set autoddl on"), Some(true));
        assert_eq!(set_auto_ddl("SET AUTO  off"), Some(false));
        assert_eq!(set_auto_ddl("SET AUTODDL"), Some(true));
        assert_eq!(set_auto_ddl("SET NAMES UTF8"), None);
        assert_eq!(set_auto_ddl("SHOW TABLES"), None);
    }
}
