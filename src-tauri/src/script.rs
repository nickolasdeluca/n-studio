//! The script executor's commands: a whole script run here, statement by
//! statement, with the UI told how far it is and what failed. Thousands of
//! statements would cost a round trip to the UI each if it ran them itself.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use fb_core::{run_script, ScriptOptions, ScriptSummary, Step, StepOutcome, TxId, TxOptions};
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

use crate::commands::{blocking, AppState};
use crate::error::AppResult;

/// The event a running script reports through
pub const PROGRESS: &str = "script-progress";

/// The scripts running, by the id the UI gave each, and how to stop them
#[derive(Default)]
pub struct Scripts(Mutex<HashMap<u64, Arc<AtomicBool>>>);

/// A statement that did not run to its end, for the UI's log
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Problem {
    /// Among the script's statements, from 0
    index: usize,
    /// In the script's text, in UTF-16 code units as the editor counts
    from: usize,
    to: usize,
    /// `None` for an isql command, passed over
    error: Option<String>,
    sqlcode: Option<i32>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    run: u64,
    /// Statements gone through so far, of `statements`
    at: usize,
    statements: usize,
    failed: usize,
    /// Since the last event
    problems: Vec<Problem>,
}

/// How a script ended, with what failed since the last [`PROGRESS`] event:
/// the answer and the events travel apart, and only the answer is sure to
/// be the last thing the UI hears
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScriptEnd {
    #[serde(flatten)]
    summary: ScriptSummary,
    problems: Vec<Problem>,
}

/// Byte offsets into a text as UTF-16 offsets, for offsets that only grow:
/// each is counted from the one before, not from the start
struct Utf16<'a> {
    text: &'a str,
    byte: usize,
    units: usize,
}

impl<'a> Utf16<'a> {
    fn new(text: &'a str) -> Self {
        Self { text, byte: 0, units: 0 }
    }

    fn at(&mut self, byte: usize) -> usize {
        let byte = byte.min(self.text.len());
        if byte < self.byte {
            (self.byte, self.units) = (0, 0);
        }
        self.units += self.text[self.byte..byte].encode_utf16().count();
        self.byte = byte;
        self.units
    }
}

/// Runs `text` as a script on a connected registration: in `tx` when the
/// caller has a transaction, in one begun with `tx_options` otherwise.
/// `run` names it for [`script_cancel`] and in its [`PROGRESS`] events,
/// which carry what failed as it fails. The summary says which transaction
/// is left active, if any, for the caller to end.
#[tauri::command]
pub async fn script_run(
    app: AppHandle,
    id: String,
    run: u64,
    tx: Option<TxId>,
    tx_options: TxOptions,
    text: String,
    options: ScriptOptions,
) -> AppResult<ScriptEnd> {
    let events = app.clone();
    blocking(app, move |state| {
        let conn = state.session(&id)?;
        let cancel = Arc::new(AtomicBool::new(false));
        state.scripts.0.lock().unwrap().insert(run, cancel.clone());

        let mut offsets = Utf16::new(&text);
        let mut pending = Progress { run, at: 0, statements: 0, failed: 0, problems: Vec::new() };
        let mut sent = Instant::now();
        let result = run_script(&conn, tx, tx_options, &text, options, &cancel, |step: &Step, statements| {
            pending.at = step.index + 1;
            pending.statements = statements;
            let problem = |error: Option<String>, sqlcode, offsets: &mut Utf16| {
                let from = offsets.at(step.range.start);
                Problem { index: step.index, from, to: offsets.at(step.range.end), error, sqlcode }
            };
            match &step.outcome {
                StepOutcome::Done { .. } => {}
                StepOutcome::Skipped => pending.problems.push(problem(None, None, &mut offsets)),
                StepOutcome::Failed { sqlcode, message } => {
                    pending.failed += 1;
                    pending.problems.push(problem(Some(message.clone()), Some(*sqlcode), &mut offsets));
                }
            }
            // Often enough to watch, seldom enough not to be the work
            if sent.elapsed() >= Duration::from_millis(100) || pending.problems.len() >= 200 {
                let _ = events.emit(PROGRESS, &pending);
                pending.problems.clear();
                sent = Instant::now();
            }
        });
        state.scripts.0.lock().unwrap().remove(&run);

        // Whatever the transaction it started in was showing is over when
        // the script ended it
        if let Some(before) = tx {
            if result.as_ref().map_or(true, |summary| summary.tx != Some(before)) {
                state.results.transaction_ended(&id, before);
            }
        }
        Ok(ScriptEnd { summary: result?, problems: pending.problems })
    })
    .await
}

/// Stops a running script: before its next statement, and the one running
/// too when the server can cancel it. `false` when no such script runs.
#[tauri::command]
pub fn script_cancel(state: State<'_, AppState>, id: String, run: u64) -> AppResult<bool> {
    let Some(cancel) = state.scripts.0.lock().unwrap().get(&run).cloned() else {
        return Ok(false);
    };
    cancel.store(true, Ordering::Relaxed);
    // A server too old to cancel still stops between statements
    let _ = state.session(&id)?.cancel();
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::Utf16;

    #[test]
    fn offsets_count_as_the_editor_does() {
        let text = "a ção 😀 b";
        let mut offsets = Utf16::new(text);
        let byte = |needle: &str| text.find(needle).unwrap();
        assert_eq!(offsets.at(byte("ção")), 2);
        assert_eq!(offsets.at(byte("😀")), 6);
        // The emoji is two units
        assert_eq!(offsets.at(byte("b")), 9);
        // Going back starts over
        assert_eq!(offsets.at(byte("ção")), 2);
        assert_eq!(offsets.at(text.len() + 10), 10);
    }
}
