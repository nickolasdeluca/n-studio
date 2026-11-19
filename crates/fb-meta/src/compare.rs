//! Two tables' rows compared, one the source and the other the target, and
//! the statements that would make the target's rows the source's.
//!
//! Rows are matched by the target's key. The two tables may sit on
//! different servers, whose orders of text need not agree, so nothing here
//! depends on order: the target is read once into a map from each row's key
//! to a digest of the row, the source is read against it, and only the
//! target rows found to differ are read again, to say in what.
//!
//! A blob is compared by a digest the server works out in the same read,
//! and read itself only for a row that differs, to write it into the
//! script. Arrays are left out: no SQL statement reads or writes one.
//! Computed columns are compared, and never written.

use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicBool, Ordering};

use fb_core::{Connection, Error, ExecuteOptions, Isolation, Result, TxEnd, TxId, TxOptions, Value};
use serde::Serialize;

use crate::data::editing;
use crate::ddl::{Context, Statement};
use crate::ident::{literal, quote};
use crate::table::{table, Column, Table};
use crate::types::Temporal;

/// Rows asked of the server at a time
const PAGE: usize = 2000;
/// Target rows read again in one statement, to see what differs in them
const BATCH: usize = 40;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiffKind {
    /// Only the source has the row: the target would get it
    OnlySource,
    /// Only the target has the row: the target would lose it
    OnlyTarget,
    /// Both have it, with other values
    Different,
}

/// A column whose value is not the same on both sides; `None` is null
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Change {
    pub column: String,
    pub source: Option<String>,
    pub target: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DiffRow {
    pub kind: DiffKind,
    /// The key's values, in the key's column order
    pub key: Vec<Option<String>>,
    /// Of a row that differs, the columns that do
    pub changes: Vec<Change>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Comparison {
    pub key: Vec<String>,
    /// The columns compared besides the key
    pub columns: Vec<String>,
    /// What was left out, and why
    pub notes: Vec<String>,
    pub source_rows: u64,
    pub target_rows: u64,
    pub same: u64,
    pub only_source: u64,
    pub only_target: u64,
    pub different: u64,
    /// The differences, as many as were asked to be kept
    pub rows: Vec<DiffRow>,
    /// There are more differences than `rows` has: the scripts are not
    /// the whole of what it takes
    pub truncated: bool,
    /// The statements that make the target's rows the source's, by what
    /// they do, each ready to run on the target
    pub deletes: String,
    pub updates: String,
    pub inserts: String,
    pub cancelled: bool,
}

/// How far a comparison is
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Progress {
    /// "target", "source" or "details"
    pub phase: &'static str,
    pub rows: u64,
}

fn invalid(message: String) -> Error {
    Error::Client { message }
}

/// A value as it is shown, and as a statement writes it
#[derive(Debug, Clone, PartialEq, Eq)]
struct Lit {
    show: Option<String>,
    sql: String,
}

/// Firebird keeps a ten-thousandth of a second, and takes no more digits
fn fraction(time: &str) -> String {
    match time.split_once('.') {
        Some((whole, frac)) => {
            let frac: String = frac.chars().take(4).collect();
            let frac = frac.trim_end_matches('0');
            if frac.is_empty() {
                whole.to_string()
            } else {
                format!("{whole}.{frac}")
            }
        }
        None => time.to_string(),
    }
}

fn lit(column: &Column, value: &Value) -> Lit {
    let quoted = |text: String| Lit { sql: literal(&text), show: Some(text) };
    let bare = |text: String| Lit { sql: text.clone(), show: Some(text) };
    match value {
        Value::Null | Value::BlobRef { .. } => Lit { show: None, sql: "NULL".into() },
        Value::Integer(i) => bare(i.to_string()),
        Value::Decimal(d) => bare(d.to_string()),
        Value::Float(f) => bare(format!("{f:?}")),
        Value::Boolean(b) => bare(if *b { "TRUE" } else { "FALSE" }.to_string()),
        // A CHAR is the same value whatever it is padded to
        Value::Text(s) if column.data_type.is_padded() => quoted(s.trim_end_matches(' ').to_string()),
        Value::Text(s) => quoted(s.clone()),
        Value::Binary(b) => {
            let hex: String = b.iter().map(|byte| format!("{byte:02X}")).collect();
            Lit { sql: format!("x'{hex}'"), show: Some(hex) }
        }
        Value::Timestamp(t) => {
            let text = t.to_string();
            let (date, time) = text.split_once(' ').unwrap_or((&text, "00:00:00"));
            quoted(match column.data_type.temporal() {
                Some(Temporal::Date) => date.to_string(),
                Some(Temporal::Time) => fraction(time),
                _ => format!("{date} {}", fraction(time)),
            })
        }
    }
}

const PART: char = '\u{1f}';

/// A row's key as one text, to find the row by
fn joined(values: &[Lit]) -> String {
    let mut out = String::new();
    for (i, v) in values.iter().enumerate() {
        if i > 0 {
            out.push(PART);
        }
        out.push_str(&v.sql);
    }
    out
}

/// A row's values as a number that two equal rows share, and two different
/// ones do not but for a chance in 2^128
fn digest(values: &[Lit]) -> u128 {
    let mut a = DefaultHasher::new();
    let mut b = DefaultHasher::new();
    0x9e37_79b9_u32.hash(&mut b);
    for v in values {
        v.sql.hash(&mut a);
        v.sql.hash(&mut b);
    }
    ((a.finish() as u128) << 64) | b.finish() as u128
}

/// A blob column, compared by a digest the server works out, so that its
/// value is read only for the rows that differ
struct Blob {
    name: String,
    /// A TEXT blob on both sides, in other character sets: compared as
    /// UTF-8, or the same text would differ in its bytes
    as_utf8: bool,
    /// The target computes it: compared, never written
    computed: bool,
}

/// What of the two tables is compared
struct Plan<'a> {
    source: &'a Table,
    target: &'a Table,
    key: Vec<String>,
    /// Compared by value, besides the key
    columns: Vec<String>,
    /// Of `columns`, those the target computes: never written
    computed: Vec<String>,
    blobs: Vec<Blob>,
    /// Both servers digest a blob with SHA-256 (Firebird 4 and later);
    /// before, there is a 64-bit hash and the length
    strong: bool,
    notes: Vec<String>,
}

fn plan<'a>(source: &'a Table, target: &'a Table, strong: bool) -> Result<Plan<'a>> {
    let Some(editing) = editing(target) else {
        return Err(invalid(format!(
            "{} has no key that tells its rows apart: a primary key, or a unique one without nulls.",
            target.name
        )));
    };
    let of_source = |name: &str| source.columns.iter().find(|c| c.name == name);
    if let Some(missing) = editing.key.iter().find(|k| of_source(k).is_none()) {
        return Err(invalid(format!("The source table {} has no column {missing}, which is in the target's key.", source.name)));
    }
    let mut out = Plan {
        source,
        target,
        key: editing.key.clone(),
        columns: Vec::new(),
        computed: Vec::new(),
        blobs: Vec::new(),
        strong,
        notes: Vec::new(),
    };
    for column in &target.columns {
        if editing.key.contains(&column.name) {
            continue;
        }
        let Some(theirs) = of_source(&column.name) else {
            out.notes.push(format!("{} is not compared: only in the target.", column.name));
            continue;
        };
        let (ours, other) = (&column.data_type, &theirs.data_type);
        let computed = column.computed.is_some();
        if ours.is_array() || other.is_array() {
            out.notes.push(format!("{} is not compared: an array, which no SQL statement reads or writes.", column.name));
            continue;
        }
        if computed {
            out.notes.push(format!("{} is compared but never written: the target computes it.", column.name));
        }
        if ours.is_stored_apart() || other.is_stored_apart() {
            let text = |t: &crate::types::DataType| t.is_text_blob();
            let set = |t: &crate::types::DataType| t.charset.clone().filter(|c| c != "NONE" && c != "OCTETS");
            let as_utf8 = text(ours) && text(other) && set(ours).is_some() && set(other).is_some() && set(ours) != set(other);
            out.blobs.push(Blob { name: column.name.clone(), as_utf8, computed });
        } else {
            out.columns.push(column.name.clone());
            if computed {
                out.computed.push(column.name.clone());
            }
        }
    }
    for column in &source.columns {
        if !target.columns.iter().any(|c| c.name == column.name) {
            out.notes.push(format!("{} is not compared: only in the source.", column.name));
        }
    }
    if !out.blobs.is_empty() && !strong {
        out.notes.push(
            "Blobs are compared by their length and a 64-bit hash, as servers before Firebird 4 offer: \
             a change that keeps both would be missed."
                .into(),
        );
    }
    Ok(out)
}

/// Bytes of one blob a script writes: past them it says to copy it by hand
fn blob_limit(major: u32) -> usize {
    // Before Firebird 3 a statement is at most 64 KB
    if major >= 3 {
        1 << 20
    } else {
        16_000
    }
}

/// Bytes of blobs a script holds in all
const BLOB_BUDGET: usize = 64 << 20;

/// Of a blob, what is read of it to show beside the other side's
const PREVIEW: usize = 256;

/// A blob's value as a script writes it and as it is shown
#[derive(Debug, Clone, PartialEq, Eq)]
enum Content {
    Null,
    Value { show: String, sql: String },
    /// Larger than a script holds: shown, not written
    TooLarge { show: String },
}

impl Content {
    fn show(&self) -> Option<String> {
        match self {
            Content::Null => None,
            Content::Value { show, .. } | Content::TooLarge { show } => Some(show.clone()),
        }
    }
}

/// A text as a blob literal: in pieces, as a string literal holds 32 KB
fn text_blob_sql(text: &str) -> String {
    const CHARS: usize = 8000;
    let mut pieces = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        let end = rest.char_indices().nth(CHARS).map_or(rest.len(), |(i, _)| i);
        pieces.push(literal(&rest[..end]));
        rest = &rest[end..];
    }
    match pieces.len() {
        0 => "''".into(),
        1 => pieces.remove(0),
        _ => format!("CAST({} AS BLOB SUB_TYPE TEXT) || {}", pieces[0], pieces[1..].join(" || ")),
    }
}

/// Bytes as a blob literal, in pieces as [`text_blob_sql`]
fn binary_blob_sql(bytes: &[u8]) -> String {
    let hex = |part: &[u8]| format!("x'{}'", part.iter().map(|b| format!("{b:02X}")).collect::<String>());
    let pieces: Vec<String> = bytes.chunks(16_000).map(hex).collect();
    match pieces.len() {
        0 => "x''".into(),
        1 => pieces[0].clone(),
        _ => format!("CAST({} AS BLOB SUB_TYPE BINARY) || {}", pieces[0], pieces[1..].join(" || ")),
    }
}

fn text_preview(text: &str, cut: bool) -> String {
    let mut shown: String = text.chars().take(120).collect();
    if cut || shown.len() < text.len() {
        shown.push('…');
    }
    shown
}

fn binary_preview(bytes: &[u8], cut: bool) -> String {
    let hex: String = bytes.iter().take(16).map(|b| format!("{b:02X}")).collect();
    let size = if cut { format!("more than {} bytes", bytes.len()) } else { format!("{} bytes", bytes.len()) };
    if bytes.len() > 16 || cut {
        format!("{hex}… ({size})")
    } else {
        format!("{hex} ({size})")
    }
}

/// Reads blobs for a script, within [`BLOB_BUDGET`]
struct Blobs {
    limit: usize,
    used: usize,
}

impl Blobs {
    /// The value a cursor of `tx` returned for a blob, read up to `max`
    /// bytes (the limit when `None`); a column that is no blob on this side
    /// comes as text or bytes
    fn read(&mut self, conn: &Connection, tx: TxId, value: &Value, max: Option<usize>) -> Result<Content> {
        let writing = max.is_none();
        let max = max.unwrap_or(self.limit).min(if writing { BLOB_BUDGET.saturating_sub(self.used) } else { usize::MAX });
        let (text, bytes, complete) = match value {
            Value::Null => return Ok(Content::Null),
            Value::Text(s) => (Some(s.clone()), Vec::new(), true),
            Value::Binary(b) => (None, b.clone(), true),
            Value::BlobRef { id, text } => {
                let read = conn.read_blob(tx, *id, *text, max)?;
                (read.text, read.bytes, read.complete)
            }
            other => (Some(lit(&Column::default(), other).show.unwrap_or_default()), Vec::new(), true),
        };
        let size = text.as_ref().map_or(bytes.len(), String::len);
        let show = match &text {
            Some(t) => text_preview(t, !complete),
            None => binary_preview(&bytes, !complete),
        };
        if !writing {
            return Ok(Content::Value { show, sql: String::new() });
        }
        if !complete || size > max {
            return Ok(Content::TooLarge { show });
        }
        self.used += size;
        let sql = match &text {
            Some(t) => text_blob_sql(t),
            None => binary_blob_sql(&bytes),
        };
        Ok(Content::Value { show, sql })
    }
}

impl Plan<'_> {
    /// The key's columns, then the compared ones
    fn names(&self) -> impl Iterator<Item = &String> {
        self.key.iter().chain(&self.columns)
    }

    /// What the server digests a blob into
    fn blob_digest(&self, blob: &Blob, dialect: u32) -> String {
        let column = quote(&blob.name, dialect);
        let value =
            if blob.as_utf8 { format!("CAST({column} AS BLOB SUB_TYPE TEXT CHARACTER SET UTF8)") } else { column };
        if self.strong {
            format!("HEX_ENCODE(CRYPT_HASH({value} USING SHA256))")
        } else {
            format!("HASH({value}) || '/' || OCTET_LENGTH({value})")
        }
    }

    /// The key, the compared columns, then each blob's digest
    fn select(&self, table: &Table, dialect: u32) -> String {
        let mut list: Vec<String> = self.names().map(|n| quote(n, dialect)).collect();
        list.extend(self.blobs.iter().map(|b| self.blob_digest(b, dialect)));
        format!("SELECT {} FROM {}", list.join(", "), quote(&table.name, dialect))
    }

    /// The key, the compared columns, each blob's digest, then the blobs
    fn select_with_blobs(&self, table: &Table, dialect: u32) -> String {
        let mut list: Vec<String> = self.names().map(|n| quote(n, dialect)).collect();
        list.extend(self.blobs.iter().map(|b| self.blob_digest(b, dialect)));
        list.extend(self.blobs.iter().map(|b| quote(&b.name, dialect)));
        format!("SELECT {} FROM {}", list.join(", "), quote(&table.name, dialect))
    }

    /// Where the values compared end in a row: the blobs come after
    fn compared(&self) -> usize {
        self.key.len() + self.columns.len() + self.blobs.len()
    }

    /// A row read by [`select`](Self::select), each value as its column
    /// (the target's: it is the target's types the statements must suit);
    /// a blob's digest as text
    fn lits(&self, row: &[Value]) -> Vec<Lit> {
        let mut out: Vec<Lit> = self
            .names()
            .zip(row)
            .map(|(name, value)| {
                let column = self.target.columns.iter().find(|c| &c.name == name).expect("a column of the plan");
                // Padded on either side, the spaces at the end say nothing:
                // a CHAR read into a VARCHAR would differ from it for ever
                let padded = self.source.columns.iter().any(|c| &c.name == name && c.data_type.is_padded());
                match value {
                    Value::Text(s) if padded => lit(column, &Value::Text(s.trim_end_matches(' ').to_string())),
                    _ => lit(column, value),
                }
            })
            .collect();
        let digests = row.iter().skip(out.len()).take(self.blobs.len());
        out.extend(digests.map(|v| lit(&Column::default(), v)));
        out
    }

    fn condition(&self, key: &[Lit], dialect: u32) -> String {
        let parts: Vec<String> = self
            .key
            .iter()
            .zip(key)
            .map(|(name, value)| match value.show {
                None => format!("{} IS NULL", quote(name, dialect)),
                Some(_) => format!("{} = {}", quote(name, dialect), value.sql),
            })
            .collect();
        parts.join(" AND ")
    }

    /// Rows found by their keys, `BATCH` to a statement
    fn of_keys(&self, select: &str, rows: &[&[Lit]], dialect: u32) -> String {
        let conditions: Vec<String> = rows.iter().map(|key| format!("({})", self.condition(key, dialect))).collect();
        format!("{select} WHERE {}", conditions.join(" OR "))
    }
}

/// Reads a query's rows a page at a time, in a snapshot of its own, which
/// `each` is given to read blobs in; `false` from it stops
fn stream(conn: &Connection, sql: &str, mut each: impl FnMut(TxId, Vec<Value>) -> Result<bool>) -> Result<()> {
    let tx = conn.begin(TxOptions { isolation: Isolation::Snapshot, read_only: true, ..Default::default() })?;
    let mut run = || -> Result<()> {
        let executed = conn.execute(tx, sql, vec![], ExecuteOptions { plan: false, performance: false })?;
        let Some(cursor) = executed.cursor else { return Ok(()) };
        loop {
            let page = conn.fetch(cursor, PAGE)?;
            for row in page.rows {
                if !each(tx, row)? {
                    return conn.close_cursor(cursor);
                }
            }
            if page.eof {
                return Ok(());
            }
        }
    };
    let result = run();
    conn.end(tx, TxEnd::Rollback).ok();
    result
}

/// A line of a script that is not run, with no line break in it
fn comment(text: String) -> Statement {
    Statement::plain(format!("-- {}", text.replace(['\r', '\n'], " ")))
}

/// Compares `source_table` on `source` with `target_table` on `target`.
/// At most `keep` differences are kept in detail and in the scripts; all
/// of them are counted.
pub fn compare(
    source: &Connection,
    source_table: &str,
    target: &Connection,
    target_table: &str,
    keep: usize,
    cancel: &AtomicBool,
    mut progress: impl FnMut(Progress),
) -> Result<Comparison> {
    let source_def = table(source, source_table)?;
    let target_def = table(target, target_table)?;
    let strong = source.info().major >= 4 && target.info().major >= 4;
    let plan = plan(&source_def, &target_def, strong)?;
    let ctx = Context::of(target.info());
    let source_dialect = source.info().dialect;
    let keys = plan.key.len();
    let stopped = || cancel.load(Ordering::Relaxed);

    let mut columns = plan.columns.clone();
    columns.extend(plan.blobs.iter().map(|b| b.name.clone()));
    let mut out = Comparison { key: plan.key.clone(), columns, notes: plan.notes.clone(), ..Default::default() };

    // The target, as its keys and what each row comes to
    let mut digests: HashMap<String, u128> = HashMap::new();
    stream(target, &plan.select(plan.target, ctx.dialect), |_, row| {
        let lits = plan.lits(&row);
        digests.insert(joined(&lits[..keys]), digest(&lits[keys..]));
        out.target_rows += 1;
        if out.target_rows % PAGE as u64 == 0 {
            progress(Progress { phase: "target", rows: out.target_rows });
        }
        Ok(!stopped())
    })?;

    // The source against it. What differs waits for the target's values.
    let mut inserts: Vec<Vec<Lit>> = Vec::new();
    let mut differing: Vec<Vec<Lit>> = Vec::new();
    let mut kept = 0usize;
    if !stopped() {
        stream(source, &plan.select(plan.source, source_dialect), |_, row| {
            let lits = plan.lits(&row);
            out.source_rows += 1;
            match digests.remove(&joined(&lits[..keys])) {
                None => {
                    out.only_source += 1;
                    if kept < keep {
                        kept += 1;
                        inserts.push(lits);
                    }
                }
                Some(d) if d == digest(&lits[keys..]) => out.same += 1,
                Some(_) => {
                    out.different += 1;
                    if kept < keep {
                        kept += 1;
                        differing.push(lits);
                    }
                }
            }
            if out.source_rows % PAGE as u64 == 0 {
                progress(Progress { phase: "source", rows: out.source_rows });
            }
            Ok(!stopped())
        })?;
    }
    if stopped() {
        out.cancelled = true;
        return Ok(out);
    }

    // What the source never mentioned is the target's alone
    out.only_target = digests.len() as u64;
    let mut only_target: Vec<Vec<Lit>> = Vec::new();
    for key in digests.into_keys() {
        if kept == keep {
            break;
        }
        kept += 1;
        // The key's values back from the text they were joined into
        let lits: Vec<Lit> = key
            .split(PART)
            .map(|sql| Lit { sql: sql.to_string(), show: (sql != "NULL").then(|| unquoted(sql)) })
            .collect();
        only_target.push(lits);
    }
    only_target.sort_by(|a, b| joined(a).cmp(&joined(b)));
    out.truncated = (out.only_source + out.only_target + out.different) as usize > kept;

    let name = quote(&plan.target.name, ctx.dialect);
    let quoted = |n: &String| quote(n, ctx.dialect);
    let mut deletes = Vec::new();
    let mut updates = Vec::new();
    let mut insert_statements = Vec::new();
    let mut blobs = Blobs { limit: blob_limit(target.info().major), used: 0 };
    let first_blob = keys + plan.columns.len();
    let too_large = |column: &str, condition: &str, limit: usize| {
        comment(format!(
            "{column} of the row where {condition} is larger than a script holds ({} KB): copy it by hand",
            limit / 1024
        ))
    };

    for key in &only_target {
        deletes.push(Statement::plain(format!("DELETE FROM {name} WHERE {}", plan.condition(key, ctx.dialect))));
        out.rows.push(DiffRow { kind: DiffKind::OnlyTarget, key: key.iter().map(|l| l.show.clone()).collect(), changes: Vec::new() });
    }

    // The target's side of the rows that differ, a batch to a statement,
    // and the source's blobs that differ from the target's
    let with_blobs = plan.select_with_blobs(plan.target, ctx.dialect);
    let source_blobs = plan.select_with_blobs(plan.source, source_dialect);
    let mut read = 0u64;
    for batch in differing.chunks(BATCH) {
        if stopped() {
            out.cancelled = true;
            break;
        }
        let batch_keys: Vec<&[Lit]> = batch.iter().map(|row| &row[..keys]).collect();
        let mut theirs: HashMap<String, (Vec<Lit>, Vec<Content>)> = HashMap::new();
        stream(target, &plan.of_keys(&with_blobs, &batch_keys, ctx.dialect), |tx, row| {
            let lits = plan.lits(&row);
            let mut shown = Vec::new();
            for value in &row[plan.compared()..] {
                shown.push(blobs.read(target, tx, value, Some(PREVIEW))?);
            }
            theirs.insert(joined(&lits[..keys]), (lits, shown));
            Ok(true)
        })?;
        // The source's blobs, of the rows where one differs
        let blob_differs = |row: &[Lit]| {
            theirs.get(&joined(&row[..keys])).is_some_and(|(t, _)| (first_blob..row.len()).any(|i| row[i].sql != t[i].sql))
        };
        let wanted: Vec<&[Lit]> = batch.iter().filter(|r| blob_differs(r)).map(|r| &r[..keys]).collect();
        let mut ours: HashMap<String, Vec<Content>> = HashMap::new();
        if !wanted.is_empty() {
            stream(source, &plan.of_keys(&source_blobs, &wanted, source_dialect), |tx, row| {
                let lits = plan.lits(&row);
                let Some((target_row, _)) = theirs.get(&joined(&lits[..keys])) else { return Ok(true) };
                let mut values = Vec::new();
                for (i, value) in row[plan.compared()..].iter().enumerate() {
                    let differs = lits[first_blob + i].sql != target_row[first_blob + i].sql;
                    values.push(if differs && !plan.blobs[i].computed {
                        blobs.read(source, tx, value, None)?
                    } else {
                        Content::Null
                    });
                }
                ours.insert(joined(&lits[..keys]), values);
                Ok(true)
            })?;
        }
        for row in batch {
            let key: Vec<Option<String>> = row[..keys].iter().map(|l| l.show.clone()).collect();
            // Gone or changed since it was read: the next comparison's to say
            let Some((target_row, target_blobs)) = theirs.get(&joined(&row[..keys])) else { continue };
            let condition = plan.condition(&row[..keys], ctx.dialect);
            let mut changes = Vec::new();
            let mut set = Vec::new();
            let mut notes = Vec::new();
            let mut computed_only = Vec::new();
            for (i, column) in plan.columns.iter().enumerate() {
                let (ours, their) = (&row[keys + i], &target_row[keys + i]);
                if ours.sql != their.sql {
                    if plan.computed.contains(column) {
                        computed_only.push(column.clone());
                    } else {
                        set.push(format!("{} = {}", quoted(column), ours.sql));
                    }
                    changes.push(Change { column: column.clone(), source: ours.show.clone(), target: their.show.clone() });
                }
            }
            let source_values = ours.get(&joined(&row[..keys]));
            for (i, blob) in plan.blobs.iter().enumerate() {
                if row[first_blob + i].sql == target_row[first_blob + i].sql {
                    continue;
                }
                let value = source_values.and_then(|v| v.get(i)).cloned().unwrap_or(Content::Null);
                match &value {
                    _ if blob.computed => computed_only.push(blob.name.clone()),
                    Content::Null => set.push(format!("{} = NULL", quoted(&blob.name))),
                    Content::Value { sql, .. } => set.push(format!("{} = {sql}", quoted(&blob.name))),
                    Content::TooLarge { .. } => notes.push(too_large(&blob.name, &condition, blobs.limit)),
                }
                let target = target_blobs.get(i).and_then(Content::show);
                changes.push(Change { column: blob.name.clone(), source: value.show(), target });
            }
            if changes.is_empty() {
                continue;
            }
            updates.extend(notes);
            if set.is_empty() {
                if !computed_only.is_empty() {
                    updates.push(comment(format!(
                        "The row where {condition} differs only in what the target computes ({}): nothing to write",
                        computed_only.join(", ")
                    )));
                }
            } else {
                updates.push(Statement::plain(format!("UPDATE {name} SET {} WHERE {condition}", set.join(", "))));
            }
            out.rows.push(DiffRow { kind: DiffKind::Different, key, changes });
        }
        read += batch.len() as u64;
        progress(Progress { phase: "details", rows: read });
    }

    // The rows to insert, with the source's blobs
    let mut inserted_blobs: HashMap<String, Vec<Content>> = HashMap::new();
    if !plan.blobs.is_empty() {
        for batch in inserts.chunks(BATCH) {
            if stopped() {
                out.cancelled = true;
                break;
            }
            let batch_keys: Vec<&[Lit]> = batch.iter().map(|row| &row[..keys]).collect();
            stream(source, &plan.of_keys(&source_blobs, &batch_keys, source_dialect), |tx, row| {
                let lits = plan.lits(&row);
                let mut values = Vec::new();
                for (i, value) in row[plan.compared()..].iter().enumerate() {
                    values.push(if plan.blobs[i].computed { Content::Null } else { blobs.read(source, tx, value, None)? });
                }
                inserted_blobs.insert(joined(&lits[..keys]), values);
                Ok(true)
            })?;
        }
    }
    for row in &inserts {
        let condition = plan.condition(&row[..keys], ctx.dialect);
        let mut names: Vec<String> = Vec::new();
        let mut values: Vec<String> = Vec::new();
        for (name, l) in plan.names().zip(row.iter()) {
            if !plan.computed.contains(name) {
                names.push(quoted(name));
                values.push(l.sql.clone());
            }
        }
        let mut changes: Vec<Change> = plan
            .columns
            .iter()
            .zip(&row[keys..])
            .map(|(column, l)| Change { column: column.clone(), source: l.show.clone(), target: None })
            .collect();
        let contents = inserted_blobs.get(&joined(&row[..keys]));
        for (i, blob) in plan.blobs.iter().enumerate() {
            let value = contents.and_then(|v| v.get(i)).cloned().unwrap_or(Content::Null);
            match &value {
                _ if blob.computed => {}
                Content::Null => {}
                Content::Value { sql, .. } => {
                    names.push(quoted(&blob.name));
                    values.push(sql.clone());
                }
                Content::TooLarge { .. } => insert_statements.push(too_large(&blob.name, &condition, blobs.limit)),
            }
            changes.push(Change { column: blob.name.clone(), source: value.show(), target: None });
        }
        insert_statements.push(Statement::plain(format!(
            "INSERT INTO {name} ({}) VALUES ({})",
            names.join(", "),
            values.join(", ")
        )));
        out.rows.push(DiffRow { kind: DiffKind::OnlySource, key: row[..keys].iter().map(|l| l.show.clone()).collect(), changes });
    }

    out.deletes = lines(&deletes);
    out.updates = lines(&updates);
    out.inserts = lines(&insert_statements);
    Ok(out)
}

/// A string literal's text back; a number is itself
fn unquoted(sql: &str) -> String {
    match sql.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')) {
        Some(inner) => inner.replace("''", "'"),
        None => sql.to_string(),
    }
}

/// One statement a line: thousands of them are not read, they are run
fn lines(statements: &[Statement]) -> String {
    let mut out = String::new();
    for s in statements {
        out.push_str(&s.sql);
        // A comment ends with its line
        out.push_str(if s.sql.starts_with("--") { "\n" } else { ";\n" });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::table::{Constraint, ConstraintKind};
    use crate::types::DataType;

    fn column(name: &str, code: i64) -> Column {
        Column { name: name.into(), data_type: DataType { code, ..Default::default() }, not_null: true, ..Default::default() }
    }

    fn keyed(name: &str, columns: Vec<Column>) -> Table {
        Table {
            name: name.into(),
            columns,
            constraints: vec![Constraint {
                name: "PK".into(),
                kind: ConstraintKind::PrimaryKey,
                columns: vec!["ID".into()],
                index: None,
                index_descending: false,
                references: None,
                check: None,
            }],
            ..Default::default()
        }
    }

    #[test]
    fn values_are_written_as_their_column_takes_them() {
        let at = "2026-03-04T05:06:07.123456".parse().unwrap();
        let of = |code: i64, value: Value| lit(&column("C", code), &value).sql;
        assert_eq!(of(12, Value::Timestamp(at)), "'2026-03-04'");
        assert_eq!(of(13, Value::Timestamp(at)), "'05:06:07.1234'");
        assert_eq!(of(35, Value::Timestamp(at)), "'2026-03-04 05:06:07.1234'");
        assert_eq!(of(35, Value::Timestamp("2026-03-04T05:06:07".parse().unwrap())), "'2026-03-04 05:06:07'");
        assert_eq!(of(37, Value::Text("it's ".into())), "'it''s '");
        assert_eq!(of(8, Value::Integer(-3)), "-3");
        assert_eq!(of(27, Value::Float(0.1)), "0.1");
        assert_eq!(of(23, Value::Boolean(true)), "TRUE");
        assert_eq!(of(37, Value::Binary(vec![0, 255])), "x'00FF'");
        assert_eq!(of(8, Value::Null), "NULL");
        // A CHAR compares without its padding
        let mut padded = column("C", 14);
        padded.data_type.charset = Some("UTF8".into());
        assert_eq!(lit(&padded, &Value::Text("AB  ".into())).sql, "'AB'");
    }

    #[test]
    fn a_key_survives_being_joined() {
        assert_eq!(unquoted("'it''s'"), "it's");
        assert_eq!(unquoted("42"), "42");
    }

    #[test]
    fn the_plan_compares_what_both_have_and_says_what_it_leaves() {
        let mut computed = column("TOTAL", 8);
        computed.computed = Some("(A * 2)".into());
        let target = keyed("T", vec![column("ID", 8), column("A", 8), column("MEMO", 261), computed.clone(), column("NEW", 8)]);
        let source = keyed("S", vec![column("ID", 8), column("A", 8), column("MEMO", 261), computed, column("OLD", 8)]);
        let p = plan(&source, &target, true).unwrap();
        assert_eq!(p.key, ["ID"]);
        // A computed column is compared, and a blob by its digest
        assert_eq!((p.columns.as_slice(), p.computed.as_slice()), (&["A".to_string(), "TOTAL".into()][..], &["TOTAL".to_string()][..]));
        assert_eq!(p.blobs.iter().map(|b| b.name.as_str()).collect::<Vec<_>>(), ["MEMO"]);
        // NEW and OLD on one side only, TOTAL never written
        assert_eq!(p.notes.len(), 3, "{:?}", p.notes);
        assert!(p.select(&target, 3).contains("HEX_ENCODE(CRYPT_HASH(MEMO USING SHA256))"), "{}", p.select(&target, 3));
        // Before Firebird 4 a weaker digest, and said to be
        let weak = plan(&source, &target, false).unwrap();
        assert_eq!(weak.notes.len(), 4);
        assert!(weak.select(&target, 3).contains("HASH(MEMO) || '/' || OCTET_LENGTH(MEMO)"));
        // An array is left out
        let mut listed = column("LIST", 8);
        listed.data_type.dimensions = vec![(1, 3)];
        let with_array = keyed("T", vec![column("ID", 8), listed.clone()]);
        let a = plan(&with_array, &with_array, true).unwrap();
        assert!(a.columns.is_empty() && a.blobs.is_empty() && a.notes[0].contains("array"), "{:?}", a.notes);

        let mut keyless = target.clone();
        keyless.constraints.clear();
        assert!(plan(&source, &keyless, true).is_err());
        let other_key = keyed("S", vec![column("CODE", 8)]);
        assert!(plan(&other_key, &target, true).is_err());
    }

    #[test]
    fn a_blob_is_written_in_pieces_a_literal_holds() {
        assert_eq!(text_blob_sql("it's"), "'it''s'");
        assert_eq!(text_blob_sql(""), "''");
        let long = "é".repeat(8001);
        let sql = text_blob_sql(&long);
        assert!(sql.starts_with("CAST('é") && sql.ends_with(" AS BLOB SUB_TYPE TEXT) || 'é'"), "{}", &sql[sql.len() - 40..]);
        assert_eq!(binary_blob_sql(&[0, 255]), "x'00FF'");
        assert_eq!(binary_blob_sql(&[]), "x''");
        let sql = binary_blob_sql(&vec![1; 16_001]);
        assert!(sql.starts_with("CAST(x'0101") && sql.ends_with(" AS BLOB SUB_TYPE BINARY) || x'01'"));
    }

    #[test]
    fn a_blob_is_shown_by_its_start() {
        assert_eq!(text_preview("short", false), "short");
        assert_eq!(text_preview("cut", true), "cut…");
        assert_eq!(text_preview(&"a".repeat(200), false).chars().count(), 121);
        assert_eq!(binary_preview(&[0xAB, 1], false), "AB01 (2 bytes)");
        assert_eq!(binary_preview(&[0; 20], true), "00000000000000000000000000000000… (more than 20 bytes)");
    }

    #[test]
    fn a_comment_ends_its_line_without_a_terminator() {
        let script = lines(&[comment("a\nb".into()), Statement::plain("DELETE FROM T")]);
        assert_eq!(script, "-- a b\nDELETE FROM T;\n");
    }

    #[test]
    fn equal_rows_come_to_the_same_and_different_ones_do_not() {
        let l = |s: &str| Lit { sql: s.into(), show: Some(s.into()) };
        assert_eq!(digest(&[l("1"), l("'a'")]), digest(&[l("1"), l("'a'")]));
        assert_ne!(digest(&[l("1"), l("'a'")]), digest(&[l("1"), l("'b'")]));
    }
}
