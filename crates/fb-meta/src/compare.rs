//! Two tables' rows compared, one the source and the other the target, and
//! the statements that would make the target's rows the source's.
//!
//! Rows are matched by the target's key. The two tables may sit on
//! different servers, whose orders of text need not agree, so nothing here
//! depends on order: the target is read once into a map from each row's key
//! to a digest of the row, the source is read against it, and only the
//! target rows found to differ are read again, to say in what.
//!
//! Blobs and arrays are left out: their values are not in the row, and
//! reading them is a round trip for each.

use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicBool, Ordering};

use fb_core::{Connection, Error, ExecuteOptions, Isolation, Result, TxEnd, TxOptions, Value};
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

/// What of the two tables is compared
struct Plan<'a> {
    source: &'a Table,
    target: &'a Table,
    key: Vec<String>,
    columns: Vec<String>,
    notes: Vec<String>,
}

fn plan<'a>(source: &'a Table, target: &'a Table) -> Result<Plan<'a>> {
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
    let mut columns = Vec::new();
    let mut notes = Vec::new();
    for column in &target.columns {
        if editing.key.contains(&column.name) {
            continue;
        }
        let reason = match of_source(&column.name) {
            None => Some("only in the target"),
            Some(_) if column.computed.is_some() => Some("computed"),
            Some(s) if column.data_type.is_stored_apart() || s.data_type.is_stored_apart() => Some("a blob or an array"),
            Some(_) => None,
        };
        match reason {
            Some(reason) => notes.push(format!("{} is not compared: {reason}.", column.name)),
            None => columns.push(column.name.clone()),
        }
    }
    for column in &source.columns {
        if !target.columns.iter().any(|c| c.name == column.name) {
            notes.push(format!("{} is not compared: only in the source.", column.name));
        }
    }
    Ok(Plan { source, target, key: editing.key, columns, notes })
}

impl Plan<'_> {
    /// The key's columns, then the compared ones
    fn names(&self) -> impl Iterator<Item = &String> {
        self.key.iter().chain(&self.columns)
    }

    fn select(&self, table: &Table, dialect: u32) -> String {
        let list: Vec<String> = self.names().map(|n| quote(n, dialect)).collect();
        format!("SELECT {} FROM {}", list.join(", "), quote(&table.name, dialect))
    }

    /// A row read by [`select`](Self::select), each value as its column
    /// (the target's: it is the target's types the statements must suit)
    fn lits(&self, row: &[Value]) -> Vec<Lit> {
        self.names()
            .zip(row)
            .map(|(name, value)| {
                let column = self.target.columns.iter().find(|c| &c.name == name).expect("a column of the plan");
                lit(column, value)
            })
            .collect()
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
}

/// Reads a query's rows a page at a time, in a snapshot of its own;
/// `false` from `each` stops it
fn stream(conn: &Connection, sql: &str, mut each: impl FnMut(Vec<Value>) -> bool) -> Result<()> {
    let tx = conn.begin(TxOptions { isolation: Isolation::Snapshot, read_only: true, ..Default::default() })?;
    let mut run = || -> Result<()> {
        let executed = conn.execute(tx, sql, vec![], ExecuteOptions { plan: false, performance: false })?;
        let Some(cursor) = executed.cursor else { return Ok(()) };
        loop {
            let page = conn.fetch(cursor, PAGE)?;
            for row in page.rows {
                if !each(row) {
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
    let plan = plan(&source_def, &target_def)?;
    let ctx = Context::of(target.info());
    let keys = plan.key.len();
    let stopped = || cancel.load(Ordering::Relaxed);

    let mut out = Comparison { key: plan.key.clone(), columns: plan.columns.clone(), notes: plan.notes.clone(), ..Default::default() };

    // The target, as its keys and what each row comes to
    let mut digests: HashMap<String, u128> = HashMap::new();
    stream(target, &plan.select(plan.target, ctx.dialect), |row| {
        let lits = plan.lits(&row);
        digests.insert(joined(&lits[..keys]), digest(&lits[keys..]));
        out.target_rows += 1;
        if out.target_rows % PAGE as u64 == 0 {
            progress(Progress { phase: "target", rows: out.target_rows });
        }
        !stopped()
    })?;

    // The source against it. What differs waits for the target's values.
    let mut inserts: Vec<Vec<Lit>> = Vec::new();
    let mut differing: Vec<Vec<Lit>> = Vec::new();
    let mut kept = 0usize;
    if !stopped() {
        let source_dialect = source.info().dialect;
        stream(source, &plan.select(plan.source, source_dialect), |row| {
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
            !stopped()
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
    let column_names: Vec<String> = plan.names().map(|n| quote(n, ctx.dialect)).collect();
    let mut deletes = Vec::new();
    let mut updates = Vec::new();
    let mut insert_statements = Vec::new();

    for key in &only_target {
        deletes.push(Statement::plain(format!("DELETE FROM {name} WHERE {}", plan.condition(key, ctx.dialect))));
        out.rows.push(DiffRow { kind: DiffKind::OnlyTarget, key: key.iter().map(|l| l.show.clone()).collect(), changes: Vec::new() });
    }

    // The target's side of the rows that differ, a batch to a statement
    let select = plan.select(plan.target, ctx.dialect);
    let mut read = 0u64;
    for batch in differing.chunks(BATCH) {
        if stopped() {
            out.cancelled = true;
            break;
        }
        let conditions: Vec<String> = batch.iter().map(|row| format!("({})", plan.condition(&row[..keys], ctx.dialect))).collect();
        let mut theirs: HashMap<String, Vec<Lit>> = HashMap::new();
        stream(target, &format!("{select} WHERE {}", conditions.join(" OR ")), |row| {
            let lits = plan.lits(&row);
            theirs.insert(joined(&lits[..keys]), lits);
            true
        })?;
        for row in batch {
            let key: Vec<Option<String>> = row[..keys].iter().map(|l| l.show.clone()).collect();
            // Gone or changed since it was read: the next comparison's to say
            let Some(target_row) = theirs.get(&joined(&row[..keys])) else { continue };
            let mut changes = Vec::new();
            let mut set = Vec::new();
            for (i, column) in plan.columns.iter().enumerate() {
                let (ours, their) = (&row[keys + i], &target_row[keys + i]);
                if ours.sql != their.sql {
                    set.push(format!("{} = {}", quote(column, ctx.dialect), ours.sql));
                    changes.push(Change { column: column.clone(), source: ours.show.clone(), target: their.show.clone() });
                }
            }
            if set.is_empty() {
                continue;
            }
            updates.push(Statement::plain(format!(
                "UPDATE {name} SET {} WHERE {}",
                set.join(", "),
                plan.condition(&row[..keys], ctx.dialect)
            )));
            out.rows.push(DiffRow { kind: DiffKind::Different, key, changes });
        }
        read += batch.len() as u64;
        progress(Progress { phase: "details", rows: read });
    }

    for row in &inserts {
        let values: Vec<&str> = row.iter().map(|l| l.sql.as_str()).collect();
        insert_statements.push(Statement::plain(format!(
            "INSERT INTO {name} ({}) VALUES ({})",
            column_names.join(", "),
            values.join(", ")
        )));
        let changes = plan
            .columns
            .iter()
            .zip(&row[keys..])
            .map(|(column, l)| Change { column: column.clone(), source: l.show.clone(), target: None })
            .collect();
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
        out.push_str(";\n");
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
        let p = plan(&source, &target).unwrap();
        assert_eq!((p.key.as_slice(), p.columns.as_slice()), (&["ID".to_string()][..], &["A".to_string()][..]));
        assert_eq!(p.notes.len(), 4, "{:?}", p.notes);

        let mut keyless = target.clone();
        keyless.constraints.clear();
        assert!(plan(&source, &keyless).is_err());
        let other_key = keyed("S", vec![column("CODE", 8)]);
        assert!(plan(&other_key, &target).is_err());
    }

    #[test]
    fn equal_rows_come_to_the_same_and_different_ones_do_not() {
        let l = |s: &str| Lit { sql: s.into(), show: Some(s.into()) };
        assert_eq!(digest(&[l("1"), l("'a'")]), digest(&[l("1"), l("'a'")]));
        assert_ne!(digest(&[l("1"), l("'a'")]), digest(&[l("1"), l("'b'")]));
    }
}
