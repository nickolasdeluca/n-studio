//! The tools that look at a database as a whole rather than at one object:
//! the diagram of its tables.

use std::path::PathBuf;

use fb_meta::diagram::{self, Diagram};
use tauri::AppHandle;

use crate::commands::blocking;
use crate::error::{AppError, AppResult};

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
