//! What the SQL editors ran, kept per registration in `history.db` (SQLite)
//! beside the registrations: the statement, when, how long it took and what
//! it failed with. A convenience like the session, so a file that cannot be
//! opened is a history that lasts this run, not an app that does not start.
//!
//! A history is somebody's statements in a plain file, so two things are
//! never written: a statement that sets a password, and one too long to be
//! something typed.

use std::path::Path;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection};
use serde::Serialize;
use tauri::AppHandle;

use crate::commands::blocking;
use crate::error::{AppError, AppResult};

/// Statements kept per registration; the oldest go first
const KEPT: i64 = 2_000;
/// Past this a statement is a script out of a file, not a line of history
const LONGEST: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    id: i64,
    /// Milliseconds since 1970, when it last ran
    at: i64,
    sql: String,
    elapsed_ms: f64,
    /// What the server said, when it failed
    error: Option<String>,
    /// Times in a row it was run
    runs: i64,
}

pub struct History(Mutex<Connection>);

impl From<rusqlite::Error> for AppError {
    fn from(e: rusqlite::Error) -> Self {
        AppError::Storage { message: format!("The query history: {e}") }
    }
}

fn prepared(conn: Connection) -> rusqlite::Result<Connection> {
    conn.execute_batch(
        "create table if not exists history (
           id integer primary key,
           registration text not null,
           at integer not null,
           sql text not null,
           elapsed_ms real not null,
           error text,
           runs integer not null default 1
         );
         create index if not exists history_by_registration on history (registration, at desc, id desc);",
    )?;
    Ok(conn)
}

impl History {
    /// Opens `history.db` in `dir`, or a history in memory when that fails
    pub fn open(dir: &Path) -> Self {
        let on_disk = std::fs::create_dir_all(dir)
            .map_err(|e| e.to_string())
            .and_then(|_| Connection::open(dir.join("history.db")).and_then(prepared).map_err(|e| e.to_string()));
        let conn = on_disk.unwrap_or_else(|e| {
            eprintln!("The query history is not kept: {e}");
            Connection::open_in_memory().and_then(prepared).expect("SQLite opens in memory")
        });
        History(Mutex::new(conn))
    }

    /// Notes that `sql` ran. Run again right after itself it is the same
    /// entry, run once more. False when it is not something to keep.
    pub fn add(&self, registration: &str, sql: &str, elapsed_ms: f64, error: Option<&str>) -> AppResult<bool> {
        self.add_at(registration, sql, elapsed_ms, error, now())
    }

    fn add_at(&self, registration: &str, sql: &str, elapsed_ms: f64, error: Option<&str>, at: i64) -> AppResult<bool> {
        let sql = sql.trim();
        if sql.is_empty() || sql.len() > LONGEST || sets_a_password(sql) {
            return Ok(false);
        }
        let mut conn = self.0.lock().unwrap();
        let tx = conn.transaction()?;
        let last: Option<(i64, String)> = tx
            .query_row(
                "select id, sql from history where registration = ?1 order by at desc, id desc limit 1",
                [registration],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map(Some)
            .or_else(|e| if e == rusqlite::Error::QueryReturnedNoRows { Ok(None) } else { Err(e) })?;
        match last {
            Some((id, same)) if same == sql => {
                tx.execute(
                    "update history set at = ?2, elapsed_ms = ?3, error = ?4, runs = runs + 1 where id = ?1",
                    params![id, at, elapsed_ms, error],
                )?;
            }
            _ => {
                tx.execute(
                    "insert into history (registration, at, sql, elapsed_ms, error) values (?1, ?2, ?3, ?4, ?5)",
                    params![registration, at, sql, elapsed_ms, error],
                )?;
                tx.execute(
                    "delete from history where registration = ?1 and id not in
                       (select id from history where registration = ?1 order by at desc, id desc limit ?2)",
                    params![registration, KEPT],
                )?;
            }
        }
        tx.commit()?;
        Ok(true)
    }

    /// A registration's entries, the latest first: those with `search` in
    /// them, when it says anything
    pub fn list(&self, registration: &str, search: &str, limit: u32) -> AppResult<Vec<Entry>> {
        // LIKE's own wildcards are searched for as themselves
        let pattern = format!("%{}%", search.trim().replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"));
        let conn = self.0.lock().unwrap();
        let mut statement = conn.prepare(
            "select id, at, sql, elapsed_ms, error, runs from history
             where registration = ?1 and sql like ?2 escape '\\'
             order by at desc, id desc limit ?3",
        )?;
        let rows = statement.query_map(params![registration, pattern, limit], |row| {
            Ok(Entry {
                id: row.get(0)?,
                at: row.get(1)?,
                sql: row.get(2)?,
                elapsed_ms: row.get(3)?,
                error: row.get(4)?,
                runs: row.get(5)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Forgets everything a registration ran
    pub fn clear(&self, registration: &str) -> AppResult<()> {
        self.0.lock().unwrap().execute("delete from history where registration = ?1", [registration])?;
        Ok(())
    }
}

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// CREATE and ALTER USER, and whatever else names a password and then a
/// literal: `password 'x'`
fn sets_a_password(sql: &str) -> bool {
    let lower = sql.to_ascii_lowercase();
    lower.match_indices("password").any(|(at, word)| {
        let before = lower[..at].chars().next_back();
        let after = lower[at + word.len()..].trim_start();
        !before.is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '$') && after.starts_with('\'')
    })
}

#[tauri::command]
pub async fn history_add(
    app: AppHandle,
    id: String,
    sql: String,
    elapsed_ms: f64,
    error: Option<String>,
) -> AppResult<bool> {
    blocking(app, move |state| state.history.add(&id, &sql, elapsed_ms, error.as_deref())).await
}

#[tauri::command]
pub async fn history_list(app: AppHandle, id: String, search: String, limit: u32) -> AppResult<Vec<Entry>> {
    blocking(app, move |state| state.history.list(&id, &search, limit)).await
}

#[tauri::command]
pub async fn history_clear(app: AppHandle, id: String) -> AppResult<()> {
    blocking(app, move |state| state.history.clear(&id)).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sqls(history: &History, registration: &str, search: &str) -> Vec<String> {
        history.list(registration, search, 100).unwrap().into_iter().map(|e| e.sql).collect()
    }

    #[test]
    fn lists_the_latest_first_each_registration_its_own() {
        let dir = tempfile::tempdir().unwrap();
        let history = History::open(dir.path());
        assert!(history.add_at("a", "select 1 from rdb$database", 1.5, None, 10).unwrap());
        assert!(history.add_at("a", "  update T set X = 1\n", 20.0, Some("lock conflict"), 20).unwrap());
        assert!(history.add_at("b", "select 2 from rdb$database", 1.0, None, 30).unwrap());

        let a = history.list("a", "", 100).unwrap();
        assert_eq!(a.iter().map(|e| e.sql.as_str()).collect::<Vec<_>>(), ["update T set X = 1", "select 1 from rdb$database"]);
        assert_eq!(a[0].error.as_deref(), Some("lock conflict"));
        assert_eq!((a[1].at, a[1].elapsed_ms, a[1].runs), (10, 1.5, 1));
        assert_eq!(sqls(&history, "b", ""), ["select 2 from rdb$database"]);

        // And is there the next time the app opens
        drop(history);
        assert_eq!(sqls(&History::open(dir.path()), "a", "").len(), 2);
    }

    #[test]
    fn a_statement_run_again_at_once_is_one_entry() {
        let dir = tempfile::tempdir().unwrap();
        let history = History::open(dir.path());
        history.add_at("a", "select 1 from rdb$database", 1.0, Some("failed"), 10).unwrap();
        history.add_at("a", "select 1 from rdb$database", 3.0, None, 20).unwrap();
        history.add_at("a", "select 2 from rdb$database", 1.0, None, 30).unwrap();
        history.add_at("a", "select 1 from rdb$database", 1.0, None, 40).unwrap();

        let list = history.list("a", "", 100).unwrap();
        assert_eq!(list.len(), 3);
        // The second run's time and outcome, not the first's
        assert_eq!((list[2].at, list[2].elapsed_ms, list[2].error.clone(), list[2].runs), (20, 3.0, None, 2));
    }

    #[test]
    fn searches_for_what_was_typed_as_it_was_typed() {
        let dir = tempfile::tempdir().unwrap();
        let history = History::open(dir.path());
        history.add_at("a", "select * from PEDIDO_ITEM", 1.0, None, 10).unwrap();
        history.add_at("a", "select * from PEDIDOXITEM where NOME like '100%'", 1.0, None, 20).unwrap();

        assert_eq!(sqls(&history, "a", "pedido").len(), 2);
        // _ and % are characters here, not wildcards
        assert_eq!(sqls(&history, "a", "pedido_item"), ["select * from PEDIDO_ITEM"]);
        assert_eq!(sqls(&history, "a", "0%"), ["select * from PEDIDOXITEM where NOME like '100%'"]);
        assert_eq!(sqls(&history, "a", "nothing"), Vec::<String>::new());
    }

    #[test]
    fn keeps_no_passwords_and_no_scripts() {
        let dir = tempfile::tempdir().unwrap();
        let history = History::open(dir.path());
        assert!(!history.add("a", "create user JOAO password 'segredo'", 1.0, None).unwrap());
        assert!(!history.add("a", "ALTER USER JOAO SET PASSWORD\n  'segredo'", 1.0, None).unwrap());
        assert!(!history.add("a", &format!("select '{}' from rdb$database", "x".repeat(LONGEST)), 1.0, None).unwrap());
        assert!(!history.add("a", "  \n", 1.0, None).unwrap());
        // A column of that name is not a password being set
        assert!(history.add("a", "select PASSWORD from USUARIO where PASSWORD_HINT = 'x'", 1.0, None).unwrap());
        assert_eq!(history.list("a", "", 100).unwrap().len(), 1);
    }

    #[test]
    fn the_oldest_go_and_a_registration_can_be_cleared() {
        let dir = tempfile::tempdir().unwrap();
        let history = History::open(dir.path());
        for i in 0..KEPT + 5 {
            history.add_at("a", &format!("select {i} from rdb$database"), 1.0, None, i).unwrap();
        }
        history.add_at("b", "select 1 from rdb$database", 1.0, None, 1).unwrap();
        let all = history.list("a", "", u32::MAX).unwrap();
        assert_eq!(all.len() as i64, KEPT);
        assert_eq!(all.last().unwrap().sql, "select 5 from rdb$database");

        history.clear("a").unwrap();
        assert!(history.list("a", "", 100).unwrap().is_empty());
        assert_eq!(history.list("b", "", 100).unwrap().len(), 1);
    }

    #[test]
    fn a_directory_it_cannot_write_to_is_a_history_in_memory() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("not-a-directory");
        std::fs::write(&file, b"").unwrap();
        let history = History::open(&file);
        assert!(history.add("a", "select 1 from rdb$database", 1.0, None).unwrap());
        assert_eq!(history.list("a", "", 100).unwrap().len(), 1);
    }
}
