//! M0 item 4: Tauri shell + streamed, virtualized grid.
//!
//! Throwaway. Answers whether a WebView grid stays responsive over a 1M-row
//! result held in a scroll-back store, and what that costs in memory. Also
//! meets the M0 exit criterion end to end: connect, SELECT, stream into the
//! grid, show the performance block.
//!
//! `FBX_BENCH=synthetic:<rows>:<cols>` or `FBX_BENCH=sql:<select>` runs the
//! measurement unattended, prints one JSON line and exits.

mod cursor;
mod db;
#[path = "../../../rowstore/rowstore.rs"]
mod rowstore;

use cursor::{synthetic_columns, Chunk, Cursor, CursorInfo, Registry, Source};
use db::{DbCursor, PerfReport};
use std::sync::Mutex;
use tauri::{AppHandle, State};

type Cursors = Mutex<Registry>;

// Commands are async so they run off the main thread: a database fetch blocks.

#[tauri::command]
async fn open_synthetic(cursors: State<'_, Cursors>, rows: usize, cols: usize) -> Result<CursorInfo, String> {
    let cursor = Cursor::new(synthetic_columns(cols), Source::Synthetic { total: rows, cols });
    Ok(cursors.lock().unwrap().insert(cursor))
}

#[tauri::command]
async fn open_query(cursors: State<'_, Cursors>, sql: String) -> Result<CursorInfo, String> {
    let (columns, db) = DbCursor::open(sql)?;
    Ok(cursors.lock().unwrap().insert(Cursor::new(columns, Source::Db(db))))
}

#[tauri::command]
async fn rows(cursors: State<'_, Cursors>, id: u64, start: usize, count: usize) -> Result<Chunk, String> {
    cursors.lock().unwrap().get(id)?.rows(start, count)
}

#[tauri::command]
async fn perf(cursors: State<'_, Cursors>, id: u64) -> Result<Option<PerfReport>, String> {
    match cursors.lock().unwrap().get(id)?.db() {
        Some(db) => db.perf().map(Some),
        None => Ok(None),
    }
}

#[tauri::command]
async fn close_cursor(cursors: State<'_, Cursors>, id: u64) -> Result<(), String> {
    cursors.lock().unwrap().close(id);
    Ok(())
}

#[tauri::command]
fn bench_config() -> Option<String> {
    std::env::var("FBX_BENCH").ok()
}

/// Prints the page's measurements, adds this process's RSS, and exits.
#[tauri::command]
fn bench_report(app: AppHandle, mut report: serde_json::Value) {
    report["shell_rss_kb"] = rss_kb().into();
    report["shell_pid"] = std::process::id().into();
    println!("BENCH {report}");
    app.exit(0);
}

/// Resident set size of this process, in KiB. The WebView's content process
/// is separate and measured from outside.
fn rss_kb() -> u64 {
    std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0)
}

fn main() {
    tauri::Builder::default()
        .manage(Cursors::default())
        .invoke_handler(tauri::generate_handler![
            open_synthetic,
            open_query,
            rows,
            perf,
            close_cursor,
            bench_config,
            bench_report
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
