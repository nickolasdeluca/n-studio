//! The SQL editor's commands: its transaction, its statements, and the rows
//! they return, kept here and read by the grid a window at a time.

use std::borrow::Cow;
use std::collections::HashMap;
use std::fs::File;
use std::io::BufWriter;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use fb_core::{
    ColumnInfo, ColumnKind, Connection, CursorId, ExecuteOptions, Performance, Records, StatementType, TxEnd, TxId,
    TxOptions, Value,
};
use serde::Serialize;
use tauri::AppHandle;

use crate::commands::{blocking, AppState};
use crate::error::{AppError, AppResult};
use crate::export::{ExportOptions, Writer};
use crate::results::{display, Cell, Rows, Sort};

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
    /// The columns' names, for an export's header
    names: Vec<String>,
    sort: Option<Sort>,
    fetch_ms: f64,
    /// The transaction ended before the rows ran out: there are no more
    ended_early: bool,
    /// The transaction ended: its blobs cannot be read any more
    tx_ended: bool,
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
            sort: self.sort,
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
    pub(crate) fn transaction_ended(&self, session: &str, tx: TxId) {
        for set in self.sets.lock().unwrap().values() {
            let mut set = set.lock().unwrap();
            if set.session == session && set.tx == tx {
                set.tx_ended = true;
                if set.cursor.take().is_some() {
                    set.ended_early = true;
                }
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
    sort: Option<Sort>,
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

/// The parameters of `sql` as the server declares them, for the editor to
/// ask their values with
#[tauri::command]
pub async fn editor_describe(app: AppHandle, id: String, tx: TxId, sql: String) -> AppResult<Vec<ColumnInfo>> {
    blocking(app, move |state| Ok(state.session(&id)?.describe_params(tx, &sql)?)).await
}

/// Runs one statement in `tx`, a value for each `?` in `params` (as text,
/// which the server converts; `None` for NULL), and, for a select, fetches
/// its first `first` rows in the same call
#[tauri::command]
pub async fn editor_execute(
    app: AppHandle,
    id: String,
    tx: TxId,
    sql: String,
    params: Vec<Option<String>>,
    options: ExecuteOptions,
    first: usize,
) -> AppResult<ExecutedView> {
    blocking(app, move |state| {
        let conn = state.session(&id)?;
        let params = params.into_iter().map(|p| p.map_or(Value::Null, Value::Text)).collect();
        let executed = conn.execute(tx, &sql, params, options)?;
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
                    names: executed.columns.iter().map(|c| c.name.clone()).collect(),
                    sort: None,
                    fetch_ms: 0.0,
                    ended_early: false,
                    tx_ended: false,
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

/// Sorts the rows by a column, or puts them back in fetch order (`None`).
/// The rows not fetched yet are fetched first: a sort of some of them would
/// show an order the rest could break.
#[tauri::command]
pub async fn result_sort(app: AppHandle, result: u64, sort: Option<Sort>) -> AppResult<ResultView> {
    blocking(app, move |state| {
        let set = state.results.get(result)?;
        let mut set = set.lock().unwrap();
        if let Some(Sort { column, .. }) = sort {
            if set.rows.kinds().get(column).is_none_or(|k| matches!(k, ColumnKind::Blob | ColumnKind::TextBlob)) {
                return Err(AppError::Invalid { message: "Blob columns cannot be sorted.".into() });
            }
            set.fetch(usize::MAX)?;
        }
        set.rows.sort(sort);
        set.sort = sort;
        Ok(set.view(result))
    })
    .await
}

/// What columns `columns` of a result add up to, over every row: the rows
/// not fetched yet are fetched first, as for a sort
#[tauri::command]
pub async fn result_summary(
    app: AppHandle,
    result: u64,
    columns: Vec<usize>,
) -> AppResult<(ResultView, Vec<crate::summary::ColumnSummary>)> {
    blocking(app, move |state| {
        let set = state.results.get(result)?;
        let mut set = set.lock().unwrap();
        let cols = set.rows.kinds().len();
        if let Some(&bad) = columns.iter().find(|&&c| c >= cols) {
            return Err(AppError::Invalid { message: format!("The result has no column {bad}.") });
        }
        set.fetch(usize::MAX)?;
        let summaries = columns.iter().map(|&c| crate::summary::summarize(&set.rows, c)).collect();
        Ok((set.view(result), summaries))
    })
    .await
}

/// What an export wrote
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportSummary {
    rows: usize,
    /// The transaction ended, so blob columns hold their markers
    blobs_unread: bool,
}

/// Largest blob an export reads whole; past it, the blob is cut there
const MAX_EXPORT_BLOB: usize = 16 * 1024 * 1024;

/// Writes every row, fetching the rest first, in the order the grid shows
/// them. Blobs are read while the transaction lasts: text as is, others as
/// hex.
#[tauri::command]
pub async fn result_export(
    app: AppHandle,
    result: u64,
    path: PathBuf,
    options: ExportOptions,
) -> AppResult<ExportSummary> {
    blocking(app, move |state| {
        let set = state.results.get(result)?;
        let mut set = set.lock().unwrap();
        set.fetch(usize::MAX)?;
        let file = File::create(&path).map_err(|e| AppError::storage(&path, e))?;
        let write = |e| AppError::storage(&path, e);
        let mut out = Writer::new(BufWriter::new(file), options, &set.names).map_err(write)?;
        let blobs = !set.tx_ended;
        let cols = set.rows.kinds().len();
        for r in 0..set.rows.len() {
            let mut cells: Vec<Option<Cow<str>>> = set.rows.row(r).map(|c| c.map(Cow::Borrowed)).collect();
            for (c, cell) in cells.iter_mut().enumerate().take(cols) {
                let Some((id, text)) = set.rows.blob(r, c).filter(|_| blobs) else { continue };
                let content = set.conn.read_blob(set.tx, id, text, MAX_EXPORT_BLOB)?;
                *cell = Some(Cow::Owned(match content.text {
                    Some(text) => text,
                    None => content.bytes.iter().fold(String::from("0x"), |mut s, b| {
                        s.push_str(&format!("{b:02X}"));
                        s
                    }),
                }));
            }
            out.row(cells.iter().map(|c| c.as_deref())).map_err(write)?;
        }
        out.finish().map_err(write)?;
        let has_blobs = set.rows.kinds().iter().any(|k| matches!(k, ColumnKind::Blob | ColumnKind::TextBlob));
        Ok(ExportSummary { rows: set.rows.len(), blobs_unread: has_blobs && !blobs })
    })
    .await
}

/// A column of a result an INSERT writes: its place, and the table's column
/// it comes from
#[derive(serde::Deserialize)]
pub struct InsertColumn {
    index: usize,
    name: String,
}

/// A result's rows as INSERT statements into `table`, of `columns` only:
/// the row at `row` as the grid shows it, or every row (fetching the rest
/// first). Values are written as the grid holds them; blobs are read in the
/// transaction that fetched them. Into the file at `path` when there is one;
/// otherwise the script comes back, up to what an editor holds.
#[tauri::command]
pub async fn result_inserts(
    app: AppHandle,
    result: u64,
    table: String,
    columns: Vec<InsertColumn>,
    row: Option<usize>,
    path: Option<PathBuf>,
) -> AppResult<crate::objects::TableInserts> {
    blocking(app, move |state| {
        let set = state.results.get(result)?;
        let mut set = set.lock().unwrap();
        match &path {
            Some(path) => {
                let mut file = BufWriter::new(File::create(path).map_err(|e| AppError::storage(path, e))?);
                write_inserts(&mut set, &table, &columns, row, &mut file, None)
            }
            None => {
                let mut text = Vec::new();
                let max = Some(crate::objects::EDITOR_SCRIPT);
                let mut done = write_inserts(&mut set, &table, &columns, row, &mut text, max)?;
                let script = String::from_utf8(text)
                    .map_err(|_| AppError::Invalid { message: "The script is not UTF-8.".into() })?;
                done.script = Some(script);
                Ok(done)
            }
        }
    })
    .await
}

/// Writes [`result_inserts`]'s script into `sink`, stopping past `max`
/// bytes when given
fn write_inserts(
    set: &mut ResultSet,
    table: &str,
    columns: &[InsertColumn],
    row: Option<usize>,
    sink: &mut dyn std::io::Write,
    max: Option<usize>,
) -> AppResult<crate::objects::TableInserts> {
    let cols = set.rows.kinds().len();
    if columns.is_empty() {
        return Err(AppError::Invalid { message: format!("No column of the result comes from {table}.") });
    }
    if let Some(bad) = columns.iter().find(|c| c.index >= cols) {
        return Err(AppError::Invalid { message: format!("The result has no column {}.", bad.index) });
    }
    if row.is_none() {
        set.fetch(usize::MAX)?;
    }
    let rows = match row {
        Some(r) if r < set.rows.len() => r..r + 1,
        Some(_) => return Err(AppError::Invalid { message: "The row is no longer in the result.".into() }),
        None => 0..set.rows.len(),
    };
    let info = set.conn.info();
    let dialect = info.dialect;
    let limit = fb_meta::inserts::blob_statement_limit(info.major);
    let name = fb_meta::quote(table, dialect);
    let names: Vec<String> = columns.iter().map(|c| fb_meta::quote(&c.name, dialect)).collect();
    let write = |e: std::io::Error| AppError::Storage { message: format!("The script could not be written: {e}") };
    let mut done = crate::objects::TableInserts { script: None, rows: 0, truncated: false };
    let mut bytes = 0usize;
    for r in rows {
        let mut list = Vec::new();
        let mut values = Vec::new();
        let mut notes = Vec::new();
        let cells: Vec<Option<String>> = set.rows.row(r).map(|c| c.map(str::to_owned)).collect();
        for (column, quoted) in columns.iter().zip(&names) {
            let kind = set.rows.kinds()[column.index];
            let value = if matches!(kind, ColumnKind::Blob | ColumnKind::TextBlob) {
                match set.rows.blob(r, column.index) {
                    None => Some("NULL".to_string()),
                    Some(_) if set.tx_ended => {
                        let note = "of the next row is left out: the transaction that read it has ended";
                        notes.push(format!("{} {note}", column.name));
                        None
                    }
                    Some((id, is_text)) => {
                        let content = set.conn.read_blob(set.tx, id, is_text, limit)?;
                        if content.complete {
                            Some(fb_meta::inserts::blob_literal(&content))
                        } else {
                            notes.push(format!(
                                "{} of the next row is larger than a script holds ({} KB): copy it by hand",
                                column.name,
                                limit / 1024
                            ));
                            None
                        }
                    }
                }
            } else {
                let shown = cells[column.index].as_deref();
                let value = fb_meta::inserts::shown_literal(kind, set.rows.binary(column.index), shown);
                if value.is_none() {
                    notes.push(format!("{} of the next row is too long for the grid: copy it by hand", column.name));
                }
                value
            };
            if let Some(value) = value {
                list.push(quoted.clone());
                values.push(value);
            }
        }
        let mut line = String::new();
        for note in notes {
            line.push_str(&format!("-- {}\n", note.replace(['\r', '\n'], " ")));
        }
        if list.is_empty() {
            line.push_str(&format!("INSERT INTO {name} DEFAULT VALUES;\n"));
        } else {
            line.push_str(&format!("INSERT INTO {name} ({}) VALUES ({});\n", list.join(", "), values.join(", ")));
        }
        sink.write_all(line.as_bytes()).map_err(write)?;
        bytes += line.len();
        done.rows += 1;
        if max.is_some_and(|max| bytes >= max) {
            done.truncated = true;
            let line = format!("-- Stopped after {} rows: save the script to a file for all of them\n", done.rows);
            sink.write_all(line.as_bytes()).map_err(write)?;
            break;
        }
    }
    sink.flush().map_err(write)?;
    Ok(done)
}

/// The rows the grid has in view
#[tauri::command]
pub async fn result_rows(app: AppHandle, result: u64, from: usize, count: usize) -> AppResult<Vec<Vec<Cell>>> {
    blocking(app, move |state| Ok(state.results.get(result)?.lock().unwrap().rows.slice(from, count))).await
}

/// A blob cell's content, as the blob viewer shows it
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BlobView {
    /// A TEXT blob's text
    text: Option<String>,
    /// Any other blob's bytes, in base64: the viewer decides what they are
    /// (a picture, text in a subtype 0 blob, or bytes to dump)
    data: Option<String>,
    /// Bytes read
    size: usize,
    /// All of it: not cut at the size asked for
    complete: bool,
}

/// Reads a cell's blob, up to `max` bytes, in the transaction that fetched
/// it. `None` for a null.
fn read_cell_blob(
    state: &AppState,
    result: u64,
    row: usize,
    col: usize,
    max: usize,
) -> AppResult<Option<fb_core::BlobContent>> {
    let set = state.results.get(result)?;
    let (conn, tx, blob) = {
        let set = set.lock().unwrap();
        if set.tx_ended {
            return Err(AppError::Invalid {
                message: "The transaction that fetched these rows has ended: run the query again to read its blobs."
                    .into(),
            });
        }
        (set.conn.clone(), set.tx, set.rows.blob(row, col))
    };
    let Some((id, text)) = blob else { return Ok(None) };
    Ok(Some(conn.read_blob(tx, id, text, max)?))
}

/// Reads the blob in a cell, up to `max` bytes, in the transaction that
/// fetched it
#[tauri::command]
pub async fn result_blob(
    app: AppHandle,
    result: u64,
    row: usize,
    col: usize,
    max: usize,
) -> AppResult<Option<BlobView>> {
    blocking(app, move |state| {
        let Some(content) = read_cell_blob(state, result, row, col, max)? else { return Ok(None) };
        let size = content.text.as_ref().map_or(content.bytes.len(), String::len);
        Ok(Some(BlobView {
            data: content.text.is_none().then(|| base64(&content.bytes)),
            text: content.text,
            size,
            complete: content.complete,
        }))
    })
    .await
}

/// The most of a blob that is written to a file: what a blob can hold on
/// the smallest page size is past it, and nothing kept in a table is.
const BLOB_FILE_MAX: usize = 1 << 30;

/// Writes the whole blob in a cell to a file: a TEXT blob as UTF-8, any
/// other as its bytes. The bytes written come back.
#[tauri::command]
pub async fn result_blob_save(app: AppHandle, result: u64, row: usize, col: usize, path: PathBuf) -> AppResult<usize> {
    blocking(app, move |state| {
        let Some(content) = read_cell_blob(state, result, row, col, BLOB_FILE_MAX)? else {
            return Err(AppError::Invalid { message: "The cell is null: there is no blob to save.".into() });
        };
        let bytes = content.text.map_or(content.bytes, String::into_bytes);
        crate::files::replace(&path, &bytes)?;
        Ok(bytes.len())
    })
    .await
}

/// Standard base64, padded: what a `data:` URL and `atob` take
fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (chunk[0] as u32) << 16 | (*chunk.get(1).unwrap_or(&0) as u32) << 8 | *chunk.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
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

/// A statement's parameters: its `:name`s turned into `?`, and each one's name
#[tauri::command]
pub async fn statement_parameters(sql: String) -> fb_sql::Parameters {
    fb_sql::parameters(&sql)
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
    fn base64_pads_what_three_bytes_do_not_fill() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64(&[0xff, 0xfe, 0xfd, 0x00]), "//79AA==");
    }

    /// A select's result, as `editor_execute` keeps it, and the columns of
    /// it that come from `table`
    fn result_of(conn: &Arc<Connection>, tx: TxId, sql: &str, table: &str) -> (ResultSet, Vec<InsertColumn>) {
        let executed = conn.execute(tx, sql, vec![], ExecuteOptions::default()).unwrap();
        let columns = executed
            .columns
            .iter()
            .enumerate()
            .filter(|(_, c)| c.relation == table && !c.field.is_empty())
            .map(|(index, c)| InsertColumn { index, name: c.field.clone() })
            .collect();
        let set = ResultSet {
            session: "test".into(),
            conn: conn.clone(),
            tx,
            cursor: executed.cursor,
            rows: Rows::new(executed.columns.iter().map(|c| c.kind).collect()),
            names: executed.columns.iter().map(|c| c.name.clone()).collect(),
            sort: None,
            fetch_ms: 0.0,
            ended_early: false,
            tx_ended: false,
            performance: None,
        };
        (set, columns)
    }

    fn script(set: &mut ResultSet, table: &str, columns: &[InsertColumn], row: Option<usize>) -> String {
        let mut out = Vec::new();
        write_inserts(set, table, columns, row, &mut out, None).unwrap();
        String::from_utf8(out).unwrap()
    }

    /// Reads only: a few rows of any table script as INSERTs of the
    /// columns selected
    #[test]
    fn a_results_rows_script_as_inserts() {
        let Some(params) = fb_core::ConnectParams::from_env() else { return };
        let conn = Arc::new(Connection::open(&params).expect("connect"));
        let tx = conn.begin(TxOptions { read_only: true, ..Default::default() }).unwrap();
        let tables = fb_meta::objects(&conn, fb_meta::ObjectKind::Table).unwrap();
        for object in tables.iter().step_by((tables.len() / 8).max(1)) {
            let quoted = fb_meta::quote(&object.name, conn.info().dialect);
            let (mut set, columns) = result_of(&conn, tx, &format!("select first 20 * from {quoted}"), &object.name);
            let text = script(&mut set, &object.name, &columns, None);
            // A text with a line break in it takes more than a line
            let statements = fb_sql::split(&text);
            assert_eq!(statements.len(), set.rows.len());
            assert!(statements.iter().all(|s| text[s.range.clone()].starts_with("INSERT INTO ")));
            // Prepared, not run: the server takes them as written
            for s in statements.iter().take(3) {
                conn.describe_params(tx, &text[s.range.clone()]).expect("the server prepares the INSERT");
            }
        }
        conn.end(tx, TxEnd::Rollback).unwrap();
    }

    /// The fixture's rows, deleted and put back by the script of a select of
    /// them, are what they were. Rolled back.
    #[test]
    fn a_results_inserts_put_its_rows_back() {
        if std::env::var("FB_FIXTURE").as_deref() != Ok("1") {
            return;
        }
        let Some(params) = fb_core::ConnectParams::from_env() else { return };
        let conn = Arc::new(Connection::open(&params).expect("connect"));
        let tx = conn.begin(TxOptions::default()).unwrap();
        let (mut set, columns) = result_of(&conn, tx, "select t.* from tipos t order by id", "TIPOS");
        let all = script(&mut set, "TIPOS", &columns, None);
        assert_eq!(fb_sql::split(&script(&mut set, "TIPOS", &columns, Some(0))).len(), 1);

        let run = |sql: &str| conn.execute(tx, sql, vec![], ExecuteOptions::default()).unwrap();
        let read = || -> Vec<Vec<Value>> {
            let sql = "select t.*, hash(c_blob_text), hash(c_blob_bin) from tipos t order by id";
            let rows = conn.fetch(run(sql).cursor.unwrap(), 1000).unwrap().rows;
            rows.into_iter().map(|r| r.into_iter().filter(|v| !matches!(v, Value::BlobRef { .. })).collect()).collect()
        };
        let before = read();
        run("delete from tipos");
        for statement in fb_sql::split(&all) {
            run(&all[statement.range]);
        }
        assert_eq!(read(), before);
        conn.end(tx, TxEnd::Rollback).unwrap();
    }

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
