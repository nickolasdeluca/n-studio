//! The editors of each database, kept while it is not connected and across
//! runs: `session.json`, beside the registrations. A database's editors
//! close when it disconnects and come back when it connects. It holds what the user typed and has nowhere else, so it
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

/// One of an editor's texts
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionPage {
    /// The name it was given, if one was
    #[serde(default)]
    title: Option<String>,
    /// The script it was opened from or saved to
    file: Option<PathBuf>,
    format: Option<Format>,
    /// Its text. None when it is the file's, unchanged: the file is read
    /// again instead.
    text: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionTab {
    /// The registration it runs on, which may since be gone
    registration: String,
    title: String,
    /// Never none
    pages: Vec<SessionPage>,
    /// Which of them was in front
    page: usize,
    /// It was the one of its registration's editors in front
    #[serde(default)]
    front: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    /// Every registration's, each one's in the order they were shown
    tabs: Vec<SessionTab>,
}

#[derive(Serialize, Deserialize)]
struct File {
    version: u32,
    #[serde(flatten)]
    session: Session,
}

/// 2: an editor has pages. 1 had one text to an editor.
const VERSION: u32 = 2;

/// What version 1 wrote: an editor was one text
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TabV1 {
    registration: String,
    title: String,
    file: Option<PathBuf>,
    format: Option<Format>,
    text: Option<String>,
}

#[derive(Deserialize)]
struct FileV1 {
    tabs: Vec<TabV1>,
    active: Option<usize>,
}

impl From<FileV1> for Session {
    fn from(old: FileV1) -> Self {
        let tabs = old
            .tabs
            .into_iter()
            .enumerate()
            .map(|(i, tab)| SessionTab {
                registration: tab.registration,
                title: tab.title,
                pages: vec![SessionPage { title: None, file: tab.file, format: tab.format, text: tab.text }],
                page: 0,
                front: old.active == Some(i),
            })
            .collect();
        Session { tabs }
    }
}

#[derive(Deserialize)]
struct Version {
    version: u32,
}

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
        match serde_json::from_slice::<Version>(&bytes).map(|v| v.version) {
            Ok(1) => serde_json::from_slice::<FileV1>(&bytes).map(Session::from).unwrap_or_default(),
            Ok(VERSION) => serde_json::from_slice::<File>(&bytes).map(|file| file.session).unwrap_or_default(),
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
        let page = |file: Option<&str>, text: Option<&str>| SessionPage {
            title: text.map(|_| "Totals".to_string()),
            file: file.map(PathBuf::from),
            format: file.map(|_| Format { encoding: Encoding::Win1252, crlf: true }),
            text: text.map(str::to_string),
        };
        Session {
            tabs: vec![
                SessionTab {
                    registration: "r1".into(),
                    title: "SQL Editor 1".into(),
                    pages: vec![page(None, Some("select 1 from rdb$database"))],
                    page: 0,
                    front: false,
                },
                SessionTab {
                    registration: "r1".into(),
                    title: "SQL Editor 2".into(),
                    pages: vec![page(None, Some("select 2 from rdb$database")), page(Some("/scripts/report.sql"), None)],
                    page: 1,
                    front: true,
                },
            ],
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
    fn a_first_version_file_is_read_as_editors_of_one_page() {
        let dir = tempfile::tempdir().unwrap();
        let v1 = br#"{"version":1,"tabs":[{"registration":"r1","title":"report.sql","file":"/scripts/report.sql",
            "format":{"encoding":"win1252","crlf":true},"text":null}],"active":0}"#;
        fs::write(dir.path().join("session.json"), v1).unwrap();
        let session = SessionStore::new(dir.path()).load();
        assert_eq!(session.tabs.len(), 1);
        assert!(session.tabs[0].front);
        assert_eq!(session.tabs[0].title, "report.sql");
        assert_eq!(session.tabs[0].page, 0);
        assert_eq!(session.tabs[0].pages[0].file, Some(PathBuf::from("/scripts/report.sql")));
        assert_eq!(session.tabs[0].pages[0].text, None);
    }

    #[test]
    fn a_file_it_cannot_read_is_an_empty_session() {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::new(dir.path());
        fs::write(dir.path().join("session.json"), b"{ not json").unwrap();
        assert_eq!(store.load(), Session::default());
        fs::write(dir.path().join("session.json"), br#"{"version":99,"tabs":[]}"#).unwrap();
        assert_eq!(store.load(), Session::default());
        // And is written over by the next one
        store.save(sample()).unwrap();
        assert_eq!(store.load(), sample());
    }
}
