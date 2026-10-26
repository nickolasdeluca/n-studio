//! The SQL editor's commands: its transaction, its statements, and the rows
//! they return, kept here and read by the grid a window at a time.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use fb_core::{
    ColumnInfo, Connection, CursorId, ExecuteOptions, Performance, Records, StatementType, TxEnd, TxId, TxOptions,
};
use serde::Serialize;
use tauri::AppHandle;

use crate::commands::blocking;
use crate::error::{AppError, AppResult};
use crate::results::{display, Cell, Rows};

/// Every open result, by an id of its own: a cursor's id is only unique
/// within its connection
#[derive(Default)]
pub struct Results {
    next: AtomicU64,
    sets: Mutex<HashMap<u64, Arc<Mutex<ResultSet>>>>,
}

struct ResultSet {
    /// The registration it was run on
    session: String,
    conn: Arc<Connection>,
    tx: TxId,
    /// Open until the rows run out or the transaction ends
    cursor: Option<CursorId>,
    rows: Rows,
    fetch_ms: f64,
    /// The transaction ended before the rows ran out: there are no more
    ended_early: bool,
    performance: Option<Performance>,
}

impl ResultSet {
    fn view(&self, id: u64) -> ResultView {
        ResultView {
            id,
            rows: self.rows.len(),
            eof: self.cursor.is_none(),
            ended_early: self.ended_early,
            fetch_ms: self.fetch_ms,
            performance: self.performance.clone(),
        }
    }

    /// Fetches up to `max` more rows, in batches so a "fetch all" does not
    /// hold every row twice
    fn fetch(&mut self, max: usize) -> AppResult<()> {
        let mut left = max;
        while left > 0 {
            let Some(cursor) = self.cursor else { break };
            let batch = match self.conn.fetch(cursor, left.min(10_000)) {
                Ok(batch) => batch,
                Err(e) => {
                    // A cursor that failed has nothing more to give
                    self.cursor = None;
                    return Err(e.into());
                }
            };
            left -= batch.rows.len();
            self.fetch_ms += batch.fetch_ms;
            for row in batch.rows {
                self.rows.push(row);
            }
            if batch.eof {
                self.cursor = None;
                self.performance = batch.performance;
            }
        }
        Ok(())
    }
}

impl Results {
    fn get(&self, id: u64) -> AppResult<Arc<Mutex<ResultSet>>> {
        self.sets
            .lock()
            .unwrap()
            .get(&id)
            .cloned()
            .ok_or_else(|| AppError::NotFound { message: "That result is closed.".into() })
    }

    /// Results of a transaction that ended keep their rows, and get no more
    fn transaction_ended(&self, session: &str, tx: TxId) {
        for set in self.sets.lock().unwrap().values() {
            let mut set = set.lock().unwrap();
            if set.session == session && set.tx == tx && set.cursor.take().is_some() {
                set.ended_early = true;
            }
        }
    }

    /// A session's results go with its connection
    pub fn close_session(&self, session: &str) {
        self.sets.lock().unwrap().retain(|_, set| set.lock().unwrap().session != session);
    }
}

/// What a statement did, for the editor's result and message tabs
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutedView {
    statement_type: StatementType,
    columns: Vec<ColumnInfo>,
    /// A select's rows, read with `result_rows`
    result: Option<ResultView>,
    /// An EXECUTE PROCEDURE's output row
    rows: Vec<Vec<Cell>>,
    records: Records,
    plan: Option<String>,
    explained_plan: Option<String>,
    prepare_ms: f64,
    execute_ms: f64,
    ended: Option<TxEnd>,
    performance: Option<Performance>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResultView {
    id: u64,
    /// Fetched so far
    rows: usize,
    /// No more to fetch
    eof: bool,
    ended_early: bool,
    fetch_ms: f64,
    /// With the last rows, when the statement ran with `performance`
    performance: Option<Performance>,
}

#[tauri::command]
pub async fn editor_begin(app: AppHandle, id: String, options: TxOptions) -> AppResult<TxId> {
    blocking(app, move |state| Ok(state.session(&id)?.begin(options)?)).await
}

#[tauri::command]
pub async fn editor_end(app: AppHandle, id: String, tx: TxId, how: TxEnd) -> AppResult<()> {
    blocking(app, move |state| {
        state.session(&id)?.end(tx, how)?;
        if how.closes() {
            state.results.transaction_ended(&id, tx);
        }
        Ok(())
    })
    .await
}

/// Cancels what the registration's connection is running: an execute or a
/// fetch fails with "operation was cancelled". `false` when nothing ran.
#[tauri::command]
pub async fn editor_cancel(app: AppHandle, id: String) -> AppResult<bool> {
    blocking(app, move |state| Ok(state.session(&id)?.cancel()?)).await
}

/// Runs one statement in `tx` and, for a select, fetches its first `first`
/// rows in the same call
#[tauri::command]
pub async fn editor_execute(
    app: AppHandle,
    id: String,
    tx: TxId,
    sql: String,
    options: ExecuteOptions,
    first: usize,
) -> AppResult<ExecutedView> {
    blocking(app, move |state| {
        let conn = state.session(&id)?;
        let executed = conn.execute(tx, &sql, options)?;
        if executed.ended.is_some_and(TxEnd::closes) {
            state.results.transaction_ended(&id, tx);
        }
        let kinds: Vec<_> = executed.columns.iter().map(|c| c.kind).collect();
        let rows =
            executed.rows.iter().map(|row| row.iter().zip(&kinds).map(|(v, k)| display(v, *k)).collect()).collect();

        let result = match executed.cursor {
            Some(cursor) => {
                let mut set = ResultSet {
                    session: id,
                    conn,
                    tx,
                    cursor: Some(cursor),
                    rows: Rows::new(kinds),
                    fetch_ms: 0.0,
                    ended_early: false,
                    performance: None,
                };
                set.fetch(first)?;
                let result_id = state.results.next.fetch_add(1, Ordering::Relaxed) + 1;
                let view = set.view(result_id);
                state.results.sets.lock().unwrap().insert(result_id, Arc::new(Mutex::new(set)));
                Some(view)
            }
            None => None,
        };

        Ok(ExecutedView {
            statement_type: executed.statement_type,
            columns: executed.columns,
            result,
            rows,
            records: executed.records,
            plan: executed.plan,
            explained_plan: executed.explained_plan,
            prepare_ms: executed.prepare_ms,
            execute_ms: executed.execute_ms,
            ended: executed.ended,
            performance: executed.performance,
        })
    })
    .await
}

/// Fetches up to `max` more rows: a page as the grid scrolls, or all of them
#[tauri::command]
pub async fn result_fetch(app: AppHandle, result: u64, max: usize) -> AppResult<ResultView> {
    blocking(app, move |state| {
        let set = state.results.get(result)?;
        let mut set = set.lock().unwrap();
        set.fetch(max)?;
        Ok(set.view(result))
    })
    .await
}

/// The rows the grid has in view
#[tauri::command]
pub async fn result_rows(app: AppHandle, result: u64, from: usize, count: usize) -> AppResult<Vec<Vec<Cell>>> {
    blocking(app, move |state| Ok(state.results.get(result)?.lock().unwrap().rows.slice(from, count))).await
}

#[tauri::command]
pub async fn result_close(app: AppHandle, result: u64) -> AppResult<()> {
    blocking(app, move |state| {
        let Some(set) = state.results.sets.lock().unwrap().remove(&result) else { return Ok(()) };
        let set = set.lock().unwrap();
        if let Some(cursor) = set.cursor {
            set.conn.close_cursor(cursor)?;
        }
        Ok(())
    })
    .await
}

/// A statement in the editor's text, in UTF-16 offsets as the editor counts
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatementRange {
    from: usize,
    to: usize,
    /// Past its terminator
    end: usize,
    kind: fb_sql::StatementKind,
    unterminated_literal: bool,
}

/// Splits the editor's text into statements, `SET TERM` aware
#[tauri::command]
pub async fn sql_statements(text: String) -> Vec<StatementRange> {
    let mut utf16 = Utf16Cursor::default();
    fb_sql::split(&text)
        .into_iter()
        .map(|s| StatementRange {
            from: utf16.at(&text, s.range.start),
            to: utf16.at(&text, s.range.end),
            end: utf16.at(&text, s.span_end),
            kind: s.kind,
            unterminated_literal: s.unterminated_literal,
        })
        .collect()
}

/// Byte offsets to UTF-16 ones in a single pass, for offsets that only grow
#[derive(Default)]
struct Utf16Cursor {
    byte: usize,
    units: usize,
}

impl Utf16Cursor {
    fn at(&mut self, text: &str, byte: usize) -> usize {
        debug_assert!(byte >= self.byte, "offsets must not go back");
        self.units += text[self.byte..byte].encode_utf16().count();
        self.byte = byte;
        self.units
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statement_ranges_count_utf16() {
        let text = "select 'ação 😀' from t;\nselect 2 from rdb$database;".to_string();
        let ranges = tauri::async_runtime::block_on(sql_statements(text.clone()));
        let units: Vec<u16> = text.encode_utf16().collect();
        let slice = |a: usize, b: usize| String::from_utf16(&units[a..b]).unwrap();
        assert_eq!(slice(ranges[0].from, ranges[0].to), "select 'ação 😀' from t");
        assert_eq!(slice(ranges[0].to, ranges[0].end), ";");
        assert_eq!(slice(ranges[1].from, ranges[1].to), "select 2 from rdb$database");
    }
}
