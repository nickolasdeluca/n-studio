//! A table's rows as the INSERT statements that would put them back: every
//! row, or the one a grid shows.
//!
//! The rows are read from the table, in a snapshot of their own, rather than
//! taken from the grid: the grid holds what it shows, which cuts long bytes
//! and leaves blobs unread. Values are written as the data comparer writes
//! them, blobs included, in pieces a string literal holds. Computed columns
//! are left out, as the server makes them; arrays too, which no statement
//! writes.

use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};

use fb_core::{BlobContent, ColumnKind, Connection, Error, Result};
use serde::Serialize;

use crate::compare::{binary_blob_sql, blob_limit, comment, lit, stream, text_blob_sql, Blobs, Content, BLOB_BUDGET};
use crate::data::{editing, written, Assign};
use crate::ddl::{Context, Statement};
use crate::ident::{literal, quote};
use crate::table::{table, Column, Table};

/// What a script of a table's rows came to
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InsertScript {
    /// Rows written
    pub rows: u64,
    /// Stopped short of the table's end: at the size asked, or cancelled
    pub truncated: bool,
    pub cancelled: bool,
}

fn invalid(message: String) -> Error {
    Error::Client { message }
}

/// The columns an INSERT gives a value
fn written_columns(table: &Table) -> Vec<&Column> {
    table.columns.iter().filter(|c| c.computed.is_none() && !c.data_type.is_array()).collect()
}

/// `WHERE …` for the one row a grid shows, from its cells as the grid shows
/// them. By the table's key when it has one; otherwise by every column a
/// value can be compared in, and the first row that matches: two rows equal
/// in all of those are the same INSERT but for a blob or a float.
fn row_condition(table: &Table, cells: &[Assign], dialect: u32) -> Result<String> {
    let cell = |name: &str| cells.iter().find(|a| a.column == name);
    let condition = |column: &Column, value: Option<&str>| match value {
        None => format!("{} IS NULL", quote(&column.name, dialect)),
        Some(v) => format!("{} = {}", quote(&column.name, dialect), written(column, Some(v))),
    };
    let mut parts = Vec::new();
    match editing(table) {
        Some(e) => {
            for name in &e.key {
                let column = table.columns.iter().find(|c| &c.name == name).expect("a key column");
                let Some(a) = cell(name) else {
                    return Err(invalid(format!("The row's {name}, in the table's key, is not shown.")));
                };
                parts.push(condition(column, a.value.as_deref()));
            }
        }
        None => {
            for column in &table.columns {
                let Some(a) = cell(&column.name) else { continue };
                // What the grid shows of it is not the value: compared, it would not match
                if column.data_type.written().is_none() || column.data_type.is_approximate() {
                    continue;
                }
                // Text the grid cut is not the value either
                if a.value.as_deref().is_some_and(|v| v.ends_with('…')) {
                    continue;
                }
                parts.push(condition(column, a.value.as_deref()));
            }
        }
    }
    if parts.is_empty() {
        return Err(invalid(format!("Nothing the grid shows tells this row of {} from the others.", table.name)));
    }
    Ok(format!("WHERE {}", parts.join(" AND ")))
}

/// Writes INSERT statements for the rows of `table_name` into `out`, one a
/// line: every row, or with `row` (a grid's cells by column) the one it
/// shows. Past `max_bytes` it stops, saying so in the script.
pub fn inserts(
    conn: &Connection,
    table_name: &str,
    row: Option<&[Assign]>,
    out: &mut dyn Write,
    max_bytes: Option<usize>,
    cancel: &AtomicBool,
) -> Result<InsertScript> {
    let table = table(conn, table_name)?;
    let ctx = Context::of(conn.info());
    let columns = written_columns(&table);
    if columns.is_empty() {
        return Err(invalid(format!("{} has no column an INSERT gives a value.", table.name)));
    }
    let name = quote(&table.name, ctx.dialect);
    let list: Vec<String> = columns.iter().map(|c| quote(&c.name, ctx.dialect)).collect();
    // GENERATED ALWAYS takes a value only when told to
    let overriding = if columns.iter().any(|c| c.identity.as_ref().is_some_and(|i| i.always)) {
        " OVERRIDING SYSTEM VALUE"
    } else {
        ""
    };
    let mut select = format!("SELECT {} FROM {name}", list.join(", "));
    if let Some(cells) = row {
        select =
            format!("SELECT FIRST 1 {} FROM {name} {}", list.join(", "), row_condition(&table, cells, ctx.dialect)?);
    }

    let io = |e: std::io::Error| invalid(format!("The script could not be written: {e}"));
    // A file holds what a blob comes to; an editor, what the budget allows
    let budget = if max_bytes.is_some() { BLOB_BUDGET } else { usize::MAX };
    let mut blobs = Blobs { limit: blob_limit(conn.info().major), used: 0, budget };
    let mut result = InsertScript::default();
    let mut bytes = 0usize;
    let line = |statement: Statement, out: &mut dyn Write| -> Result<usize> {
        let end = if statement.sql.starts_with("--") { "\n" } else { ";\n" };
        out.write_all(statement.sql.as_bytes()).map_err(io)?;
        out.write_all(end.as_bytes()).map_err(io)?;
        Ok(statement.sql.len() + end.len())
    };
    stream(conn, &select, |tx, values| {
        let mut names = Vec::new();
        let mut sql = Vec::new();
        let mut notes = Vec::new();
        for ((column, value), quoted) in columns.iter().zip(&values).zip(&list) {
            if column.data_type.is_stored_apart() {
                match blobs.read(conn, tx, value, None)? {
                    Content::Null => {}
                    Content::Value { sql: v, .. } => {
                        names.push(quoted.clone());
                        sql.push(v);
                    }
                    Content::TooLarge { .. } => notes.push(comment(format!(
                        "{} of the next row is larger than a script holds ({} KB): copy it by hand",
                        column.name,
                        blobs.limit / 1024
                    ))),
                }
            } else {
                names.push(quoted.clone());
                sql.push(lit(column, value).sql);
            }
        }
        for note in notes {
            bytes += line(note, out)?;
        }
        let statement = if names.is_empty() {
            format!("INSERT INTO {name} DEFAULT VALUES")
        } else {
            format!("INSERT INTO {name} ({}){overriding} VALUES ({})", names.join(", "), sql.join(", "))
        };
        bytes += line(Statement::plain(statement), out)?;
        result.rows += 1;
        if cancel.load(Ordering::Relaxed) {
            result.cancelled = true;
            return Ok(false);
        }
        if max_bytes.is_some_and(|max| bytes >= max) {
            result.truncated = true;
            return Ok(false);
        }
        Ok(true)
    })?;
    if row.is_some() && result.rows == 0 {
        return Err(invalid(format!("The row is no longer in {}: refresh the data and try again.", table.name)));
    }
    if result.truncated {
        let text = format!("Stopped after {} rows: save the script to a file for all of them", result.rows);
        line(comment(text), out)?;
    }
    out.flush().map_err(io)?;
    Ok(result)
}

/// A value as a result grid shows it, as the literal an INSERT writes:
/// `None` when the grid cut it and the value is not all there. `binary`:
/// the column holds bytes, shown as `0x…`.
pub fn shown_literal(kind: ColumnKind, binary: bool, shown: Option<&str>) -> Option<String> {
    let Some(shown) = shown else { return Some("NULL".into()) };
    if binary {
        if shown.ends_with('…') {
            return None;
        }
        return Some(format!("x'{}'", shown.strip_prefix("0x").unwrap_or(shown)));
    }
    Some(match kind {
        ColumnKind::Integer | ColumnKind::Decimal | ColumnKind::Boolean => shown.to_string(),
        // Infinity and NaN are no number a statement takes
        ColumnKind::Float if shown.parse::<f64>().is_ok_and(f64::is_finite) => shown.to_string(),
        _ => literal(shown),
    })
}

/// A blob's content as the literal an INSERT writes, in pieces a string
/// literal holds
pub fn blob_literal(content: &BlobContent) -> String {
    match &content.text {
        Some(text) => text_blob_sql(text),
        None => binary_blob_sql(&content.bytes),
    }
}

/// The most of one blob a statement holds, on a server of version `major`
pub fn blob_statement_limit(major: u32) -> usize {
    blob_limit(major)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::table::{Constraint, ConstraintKind};
    use crate::types::DataType;

    fn column(name: &str, code: i64) -> Column {
        Column { name: name.into(), data_type: DataType { code, ..Default::default() }, ..Default::default() }
    }

    fn cells(values: &[(&str, Option<&str>)]) -> Vec<Assign> {
        values.iter().map(|(c, v)| Assign { column: c.to_string(), value: v.map(str::to_string) }).collect()
    }

    // rdb$field_type codes
    const INTEGER: i64 = 8;
    const DOUBLE: i64 = 27;
    const VARCHAR: i64 = 37;
    const BLOB: i64 = 261;

    #[test]
    fn shown_values_are_written_as_their_kind_takes_them() {
        let w = |kind, binary, shown: Option<&str>| shown_literal(kind, binary, shown);
        assert_eq!(w(ColumnKind::Integer, false, Some("-12")).unwrap(), "-12");
        assert_eq!(w(ColumnKind::Decimal, false, None).unwrap(), "NULL");
        assert_eq!(w(ColumnKind::Text, false, Some("it's")).unwrap(), "'it''s'");
        let at = "2026-01-31 13:05:09.25";
        assert_eq!(w(ColumnKind::Timestamp, false, Some(at)).unwrap(), format!("'{at}'"));
        assert_eq!(w(ColumnKind::Float, false, Some("NaN")).unwrap(), "'NaN'");
        assert_eq!(w(ColumnKind::Text, true, Some("0x0AFF")).unwrap(), "x'0AFF'");
        assert_eq!(w(ColumnKind::Text, true, Some("0x0AFF…")), None);
    }

    #[test]
    fn a_row_is_found_by_its_key() {
        let mut t = Table {
            name: "NEWS".into(),
            columns: vec![column("ID", INTEGER), column("TEXT", VARCHAR)],
            ..Default::default()
        };
        t.columns[0].not_null = true;
        t.constraints.push(Constraint {
            name: "PK".into(),
            kind: ConstraintKind::PrimaryKey,
            columns: vec!["ID".into()],
            index: None,
            index_descending: false,
            references: None,
            check: None,
        });
        let row = cells(&[("ID", Some("3")), ("TEXT", Some("it's new"))]);
        assert_eq!(row_condition(&t, &row, 3).unwrap(), "WHERE ID = 3");
    }

    #[test]
    fn without_a_key_a_row_is_found_by_what_compares() {
        let t = Table {
            name: "LOG".into(),
            columns: vec![
                column("SEQ", INTEGER),
                column("WHAT", VARCHAR),
                column("RATE", DOUBLE),
                column("BODY", BLOB),
            ],
            ..Default::default()
        };
        let row = cells(&[("SEQ", Some("7")), ("WHAT", None), ("RATE", Some("0.1")), ("BODY", Some("(MEMO)"))]);
        assert_eq!(row_condition(&t, &row, 3).unwrap(), "WHERE SEQ = 7 AND WHAT IS NULL");
        // Only what cannot be compared: nothing to find it by
        let row = cells(&[("RATE", Some("0.1")), ("BODY", Some("(MEMO)"))]);
        assert!(row_condition(&t, &row, 3).is_err());
    }
}
