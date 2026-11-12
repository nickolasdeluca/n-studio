//! Catalog rows, and the catalog's text blobs read in line.
//!
//! A blob column costs round trips of its own for every row that has one, so
//! the queries here ask for the start of each as a string, which comes with
//! the row, and go back for the whole text only when it did not fit.

use fb_core::{Connection, Result, Value};

/// How many characters of a text blob come with its row
pub(crate) const INLINE: usize = 2000;

/// The start of a text blob as a string. The metadata character set keeps
/// the cast from failing on a database whose default cannot hold the text.
pub(crate) fn inline(expr: &str) -> String {
    format!("cast(substring({expr} from 1 for {INLINE}) as varchar({INLINE}) character set unicode_fss)")
}

pub(crate) struct Row(Vec<Value>);

impl Row {
    /// A name or a text, without the padding of a CHAR; `None` when null or
    /// empty
    pub fn text(&self, i: usize) -> Option<String> {
        let s = self.raw(i)?;
        let s = s.trim_end();
        (!s.is_empty()).then(|| s.to_string())
    }

    /// An [`inline`] text, and whether there is more of it on the server
    pub fn lob(&self, i: usize) -> (Option<String>, bool) {
        let cut = self.raw(i).is_some_and(|s| s.chars().count() >= INLINE);
        (self.text(i), cut)
    }

    pub fn int(&self, i: usize) -> Option<i64> {
        self.0.get(i)?.as_i64()
    }

    /// A flag column: null and 0 are both "no". The catalog's flags are
    /// numbers; the security database's (Firebird 3) are booleans.
    pub fn flag(&self, i: usize) -> bool {
        match self.0.get(i) {
            Some(Value::Boolean(on)) => *on,
            _ => self.int(i).unwrap_or(0) != 0,
        }
    }

    pub fn float(&self, i: usize) -> Option<f64> {
        match self.0.get(i)? {
            Value::Float(f) => Some(*f),
            v => v.as_i64().map(|i| i as f64),
        }
    }

    fn raw(&self, i: usize) -> Option<std::borrow::Cow<'_, str>> {
        match self.0.get(i)? {
            Value::Text(s) => Some(s.as_str().into()),
            Value::Binary(b) => Some(String::from_utf8_lossy(b)),
            _ => None,
        }
    }
}

pub(crate) fn rows(conn: &Connection, sql: &str, params: Vec<Value>) -> Result<Vec<Row>> {
    Ok(conn.query(sql, params)?.rows.into_iter().map(Row).collect())
}

/// The whole of a text an [`inline`] column cut short: `sql` selects the
/// blob itself, for one row. Passes `text` through when it was not cut.
pub(crate) fn whole(
    conn: &Connection,
    (text, cut): (Option<String>, bool),
    sql: &str,
    params: &[&str],
) -> Result<Option<String>> {
    if !cut {
        return Ok(text);
    }
    let params = params.iter().map(|p| Value::from(*p)).collect();
    Ok(rows(conn, sql, params)?.first().and_then(|r| r.text(0)).or(text))
}
