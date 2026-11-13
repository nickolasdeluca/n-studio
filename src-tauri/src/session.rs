//! The editors that were open, kept across runs: `session.json`, beside the
//! registrations. It holds what the user typed and has nowhere else, so it
//! is replaced whole and never half-written; and it is a convenience, so a
//! file that cannot be read is an empty session and not an app that does
//! not start.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, State};

use crate::commands::{blocking, AppState};
use crate::error::AppResult;
use crate::files::{self, Format};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionTab {
    /// The registration it runs on, which may since be gone
    registration: String,
    title: String,
    /// The script it was opened from or saved to
    file: Option<PathBuf>,
    format: Option<Format>,
    /// Its text. None when it is the file's, unchanged: the file is read
    /// again instead.
    text: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    tabs: Vec<SessionTab>,
    /// Which of them was in front
    active: Option<usize>,
}

#[derive(Serialize, Deserialize)]
struct File {
    version: u32,
    #[serde(flatten)]
    session: Session,
}

const VERSION: u32 = 1;

pub struct SessionStore {
    path: PathBuf,
    /// One write at a time: they share the temporary file
    writing: Mutex<()>,
}

impl SessionStore {
    pub fn new(dir: &Path) -> Self {
        Self { path: dir.join("session.json"), writing: Mutex::default() }
    }

    fn load(&self) -> Session {
        let Ok(bytes) = fs::read(&self.path) else { return Session::default() };
        match serde_json::from_slice::<File>(&bytes) {
            Ok(file) if file.version <= VERSION => file.session,
            _ => Session::default(),
        }
    }

    fn save(&self, session: Session) -> AppResult<()> {
        let json = serde_json::to_vec(&File { version: VERSION, session }).expect("a session serializes");
        let _one = self.writing.lock().unwrap();
        files::replace(&self.path, &json)
    }
}

#[tauri::command]
pub fn session_load(state: State<'_, AppState>) -> Session {
    state.session_store.load()
}

#[tauri::command]
pub async fn session_save(app: AppHandle, session: Session) -> AppResult<()> {
    blocking(app, move |state| state.session_store.save(session)).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::files::Encoding;

    fn sample() -> Session {
        Session {
            tabs: vec![
                SessionTab {
                    registration: "r1".into(),
                    title: "SQL Editor 1".into(),
                    file: None,
                    format: None,
                    text: Some("select 1 from rdb$database".into()),
                },
                SessionTab {
                    registration: "r1".into(),
                    title: "report.sql".into(),
                    file: Some(PathBuf::from("/scripts/report.sql")),
                    format: Some(Format { encoding: Encoding::Win1252, crlf: true }),
                    text: None,
                },
            ],
            active: Some(1),
        }
    }

    #[test]
    fn saves_and_reloads() {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::new(dir.path());
        assert_eq!(store.load(), Session::default());
        store.save(sample()).unwrap();
        assert_eq!(SessionStore::new(dir.path()).load(), sample());
    }

    #[test]
    fn a_file_it_cannot_read_is_an_empty_session() {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::new(dir.path());
        fs::write(dir.path().join("session.json"), b"{ not json").unwrap();
        assert_eq!(store.load(), Session::default());
        fs::write(dir.path().join("session.json"), br#"{"version":99,"tabs":[],"active":null}"#).unwrap();
        assert_eq!(store.load(), Session::default());
        // And is written over by the next one
        store.save(sample()).unwrap();
        assert_eq!(store.load(), sample());
    }
}
