//! The code formatter's style: `formatter.json`, beside the registrations,
//! and the file a team passes around so that everyone formats alike. Both
//! are the same JSON; a field a file lacks takes its default, so a style
//! from an older version still reads.

use std::fs;
use std::path::{Path, PathBuf};

use fb_sql::format::{format, Style};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, State};

use crate::commands::{blocking, AppState};
use crate::error::{AppError, AppResult};
use crate::files;

/// What says a file is a style, and not some other JSON
const KIND: &str = "n-studio-sql-style";
const VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
struct File {
    format: String,
    version: u32,
    #[serde(flatten)]
    style: Style,
}

pub struct StyleStore {
    path: PathBuf,
}

impl StyleStore {
    pub fn new(dir: &Path) -> Self {
        Self { path: dir.join("formatter.json") }
    }

    /// The saved style, or the default one when none was saved or the file
    /// cannot be read: formatting is never what keeps the app from working
    fn load(&self) -> Style {
        read(&self.path).unwrap_or_default()
    }
}

fn read(path: &Path) -> AppResult<Style> {
    let bytes = fs::read(path).map_err(|e| AppError::storage(path, e))?;
    let not_a_style = || AppError::Invalid { message: "This file is not an N Studio formatting style.".into() };
    let file: File = serde_json::from_slice(&bytes).map_err(|_| not_a_style())?;
    if file.format != KIND {
        return Err(not_a_style());
    }
    if file.version > VERSION {
        return Err(AppError::Invalid {
            message: "This formatting style was saved by a newer version of N Studio.".into(),
        });
    }
    Ok(file.style)
}

fn write(path: &Path, style: Style) -> AppResult<()> {
    let file = File { format: KIND.into(), version: VERSION, style };
    let mut json = serde_json::to_vec_pretty(&file).expect("a style serializes");
    json.push(b'\n');
    files::replace(path, &json)
}

/// `text` laid out by `style`. The text itself when it cannot be followed.
#[tauri::command]
pub async fn format_sql(text: String, style: Style) -> String {
    format(&text, &style)
}

#[tauri::command]
pub fn format_style_load(state: State<'_, AppState>) -> Style {
    state.style_store.load()
}

/// The style a new installation formats by
#[tauri::command]
pub fn format_style_default() -> Style {
    Style::default()
}

#[tauri::command]
pub async fn format_style_save(app: AppHandle, style: Style) -> AppResult<()> {
    blocking(app, move |state| write(&state.style_store.path, style)).await
}

/// Writes a style to a file of the user's choosing, to hand to someone else
#[tauri::command]
pub async fn format_style_export(app: AppHandle, path: PathBuf, style: Style) -> AppResult<()> {
    blocking(app, move |_| write(&path, style)).await
}

/// Reads a style someone else exported. It is not saved: the dialog shows
/// it first.
#[tauri::command]
pub async fn format_style_import(app: AppHandle, path: PathBuf) -> AppResult<Style> {
    blocking(app, move |_| read(&path)).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use fb_sql::format::Case;

    #[test]
    fn a_style_goes_to_a_file_and_comes_back() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("team.json");
        let style = Style { keywords: Case::Upper, indent: 4, right_margin: 120, ..Style::default() };
        write(&path, style.clone()).unwrap();
        assert_eq!(read(&path).unwrap(), style);
        // Readable by whoever opens it: one setting a line
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("\"format\": \"n-studio-sql-style\""));
        assert!(text.contains("\"keywords\": \"upper\""));
    }

    #[test]
    fn a_file_with_some_settings_takes_the_rest_from_the_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("partial.json");
        fs::write(&path, r#"{"format":"n-studio-sql-style","version":1,"indent":3}"#).unwrap();
        assert_eq!(read(&path).unwrap(), Style { indent: 3, ..Style::default() });
    }

    #[test]
    fn what_is_not_a_style_is_refused_and_no_saved_style_is_the_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("other.json");
        for content in [r#"{"indent":3}"#, r#"{"format":"something-else","version":1}"#, "select 1", r#"[1]"#] {
            fs::write(&path, content).unwrap();
            assert!(read(&path).is_err(), "{content}");
        }
        fs::write(&path, r#"{"format":"n-studio-sql-style","version":99}"#).unwrap();
        assert!(read(&path).is_err());
        fs::write(&path, r#"{"format":"n-studio-sql-style","version":1,"keywords":"sideways"}"#).unwrap();
        assert!(read(&path).is_err());

        let store = StyleStore::new(dir.path());
        assert_eq!(store.load(), Style::default());
        fs::write(dir.path().join("formatter.json"), "not json").unwrap();
        assert_eq!(store.load(), Style::default());
    }
}
