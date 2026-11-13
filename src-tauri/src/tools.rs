//! The tools that look at a database as a whole rather than at one object:
//! the diagram of its tables, two tables' rows compared, and a query put
//! together from parts.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use fb_meta::compare::{self, Comparison};
use fb_meta::diagram::{self, Diagram};
use fb_meta::query::{self, QuerySpec};
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

use crate::commands::{blocking, AppState};
use crate::error::{AppError, AppResult};

/// The event a running comparison says how far it is through
pub const COMPARE_PROGRESS: &str = "compare-progress";

/// The differences a comparison keeps in detail and writes statements for:
/// past this it is a migration, not a look at what differs
const KEEP: usize = 20_000;

/// The comparisons running, by the id the UI gave each, and how to stop them
#[derive(Default)]
pub struct Comparisons(Mutex<HashMap<u64, Arc<AtomicBool>>>);

#[derive(Debug, Clone, Serialize)]
struct CompareProgress {
    run: u64,
    phase: &'static str,
    rows: u64,
}

/// The diagram of `tables` and of what is within `depth` references of
/// them; of the whole database when no table is named
#[tauri::command]
pub async fn diagram(app: AppHandle, id: String, tables: Vec<String>, depth: u32) -> AppResult<Diagram> {
    blocking(app, move |state| {
        let conn = state.session(&id)?;
        Ok(diagram::diagram(&conn, &tables, depth)?)
    })
    .await
}

/// Writes a text the UI made (a diagram as SVG) where the save dialog said
#[tauri::command]
pub async fn write_text_file(app: AppHandle, path: PathBuf, text: String) -> AppResult<()> {
    blocking(app, move |_| std::fs::write(&path, text).map_err(|e| AppError::storage(&path, e))).await
}

/// Compares a table's rows on one connected registration (the source)
/// with a table's on another, or on the same (the target). Nothing is
/// changed: the answer has the statements that would make the target's
/// rows the source's. `run` names it for [`data_compare_cancel`] and in
/// its [`COMPARE_PROGRESS`] events.
#[tauri::command]
pub async fn data_compare(
    app: AppHandle,
    run: u64,
    source: String,
    source_table: String,
    target: String,
    target_table: String,
) -> AppResult<Comparison> {
    let events = app.clone();
    blocking(app, move |state| {
        let source_conn = state.session(&source)?;
        let target_conn = state.session(&target)?;
        if source == target && source_table.trim() == target_table.trim() {
            return Err(AppError::Invalid { message: "That is the same table on both sides.".into() });
        }
        let cancel = Arc::new(AtomicBool::new(false));
        state.comparisons.0.lock().unwrap().insert(run, cancel.clone());
        let result = compare::compare(&source_conn, source_table.trim(), &target_conn, target_table.trim(), KEEP, &cancel, |p| {
            let _ = events.emit(COMPARE_PROGRESS, CompareProgress { run, phase: p.phase, rows: p.rows });
        });
        state.comparisons.0.lock().unwrap().remove(&run);
        result.map_err(|e| match e {
            // What it cannot compare is the user's to put right, not a broken connection
            fb_core::Error::Client { message } => AppError::Invalid { message },
            e => e.into(),
        })
    })
    .await
}

/// Stops a running comparison at its next page of rows. `false` when no
/// such comparison runs.
#[tauri::command]
pub fn data_compare_cancel(state: State<'_, AppState>, run: u64) -> bool {
    match state.comparisons.0.lock().unwrap().get(&run) {
        Some(cancel) => {
            cancel.store(true, Ordering::Relaxed);
            true
        }
        None => false,
    }
}

/// The SELECT a builder's parts make, written for the registration's
/// database: its dialect decides how names are quoted
#[tauri::command]
pub fn query_build(state: State<'_, AppState>, id: String, spec: QuerySpec) -> AppResult<String> {
    Ok(query::select(&spec, state.session(&id)?.info().dialect))
}

/// The tables and views a query is being built from: their columns and
/// the foreign keys between them, and the name of every other one
#[tauri::command]
pub async fn query_sources(app: AppHandle, id: String, names: Vec<String>) -> AppResult<Diagram> {
    blocking(app, move |state| {
        let conn = state.session(&id)?;
        Ok(diagram::sources(&conn, &names)?)
    })
    .await
}
