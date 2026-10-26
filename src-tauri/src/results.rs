//! A query's rows as the grid shows them, kept here rather than in the UI:
//! the grid asks for the rows in view, and only those cross the bridge.
//!
//! Rows are stored as display text, a page at a time in one buffer plus an
//! offset per cell: a heap allocation per cell cost ~45 bytes of overhead
//! each, 1.3 GB for 1M rows x 30 columns (see docs/M0-FINDINGS.md).

use chrono::{NaiveDateTime, Timelike};
use fb_core::{ColumnKind, Value};

/// Rows per page
pub const PAGE: usize = 200;

/// A cell as the grid shows it: display text, or null
pub type Cell = Option<String>;

/// Up to `PAGE` rows: every cell's text back to back in `text`, and the end
/// offset of each cell in `ends` (row-major). A null takes no text and stores
/// the previous cell's end with `NULL_BIT` set, so every cell's start is the
/// previous entry's end: O(1) access.
struct Page {
    text: String,
    ends: Vec<u32>,
    rows: usize,
}

const NULL_BIT: u32 = 1 << 31;

impl Page {
    fn new(cols: usize) -> Self {
        Page { text: String::new(), ends: Vec::with_capacity(PAGE * cols), rows: 0 }
    }

    fn push(&mut self, row: impl Iterator<Item = Cell>) {
        for cell in row {
            match cell {
                Some(c) => {
                    self.text.push_str(&c);
                    // Past 2 GiB a page's offsets would collide with the null
                    // bit; cells are capped, so only thousands of wide columns get here
                    assert!(self.text.len() < NULL_BIT as usize, "page text past 2 GiB");
                    self.ends.push(self.text.len() as u32);
                }
                None => self.ends.push(self.text.len() as u32 | NULL_BIT),
            }
        }
        self.rows += 1;
    }

    fn cell(&self, i: usize) -> Option<&str> {
        let end = self.ends[i];
        if end & NULL_BIT != 0 {
            return None;
        }
        let start = if i == 0 { 0 } else { self.ends[i - 1] & !NULL_BIT };
        Some(&self.text[start as usize..end as usize])
    }
}

/// The rows fetched so far, in pages
pub struct Rows {
    kinds: Vec<ColumnKind>,
    pages: Vec<Page>,
    len: usize,
}

impl Rows {
    pub fn new(kinds: Vec<ColumnKind>) -> Self {
        Self { kinds, pages: Vec::new(), len: 0 }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn push(&mut self, row: Vec<Value>) {
        if self.pages.last().is_none_or(|p| p.rows == PAGE) {
            self.pages.push(Page::new(self.kinds.len()));
        }
        let kinds = &self.kinds;
        let page = self.pages.last_mut().expect("pushed above");
        page.push(row.iter().zip(kinds).map(|(v, k)| display(v, *k)));
        self.len += 1;
    }

    /// Rows `from..from + count`, as many as there are
    pub fn slice(&self, from: usize, count: usize) -> Vec<Vec<Cell>> {
        let cols = self.kinds.len();
        (from..(from.saturating_add(count)).min(self.len))
            .map(|r| {
                let page = &self.pages[r / PAGE];
                let at = (r % PAGE) * cols;
                (0..cols).map(|c| page.cell(at + c).map(str::to_owned)).collect()
            })
            .collect()
    }
}

/// Longest a cell's text gets: past that the grid has nothing to show, and a
/// text blob can run to megabytes
const MAX_CELL: usize = 32 * 1024;
/// Bytes shown of a binary value
const MAX_BYTES: usize = 64;

/// How a value reads in the grid, by the column it is in
pub fn display(value: &Value, kind: ColumnKind) -> Cell {
    Some(match value {
        Value::Null => return None,
        Value::Integer(i) => i.to_string(),
        Value::Decimal(d) => d.to_string(),
        Value::Float(f) => f.to_string(),
        Value::Text(s) if s.len() > MAX_CELL => {
            let mut end = MAX_CELL;
            while !s.is_char_boundary(end) {
                end -= 1;
            }
            format!("{}…", &s[..end])
        }
        Value::Text(s) => s.clone(),
        Value::Binary(b) => {
            let hex: String = b.iter().take(MAX_BYTES).map(|b| format!("{b:02X}")).collect();
            let more = if b.len() > MAX_BYTES { "…" } else { "" };
            format!("0x{hex}{more}")
        }
        Value::Boolean(b) => if *b { "TRUE" } else { "FALSE" }.to_string(),
        Value::Timestamp(t) => match kind {
            ColumnKind::Date => t.format("%Y-%m-%d").to_string(),
            ColumnKind::Time => format!("{}{}", t.format("%H:%M:%S"), fraction(t)),
            _ => format!("{}{}", t.format("%Y-%m-%d %H:%M:%S"), fraction(t)),
        },
    })
}

/// Firebird keeps time to a ten-thousandth of a second: shown when there is
/// any, without trailing zeros
fn fraction(t: &NaiveDateTime) -> String {
    let units = t.nanosecond() / 100_000;
    if units == 0 {
        return String::new();
    }
    format!(".{units:04}").trim_end_matches('0').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;
    use fb_core::Decimal;

    #[test]
    fn rows_round_trip_across_pages() {
        let mut rows = Rows::new(vec![ColumnKind::Integer, ColumnKind::Text]);
        for i in 0..PAGE * 2 + 3 {
            let text = if i % 3 == 0 { Value::Null } else { Value::Text(format!("ação {i}")) };
            rows.push(vec![Value::Integer(i as i64), text]);
        }
        assert_eq!(rows.len(), PAGE * 2 + 3);
        let slice = rows.slice(PAGE - 1, 3);
        assert_eq!(slice[0], vec![Some((PAGE - 1).to_string()), Some(format!("ação {}", PAGE - 1))]);
        assert_eq!(slice[1], vec![Some(PAGE.to_string()), Some(format!("ação {PAGE}"))]);
        assert_eq!(slice[2][1], None, "{} is a multiple of 3", PAGE + 1);
        // Past the end: what there is
        assert_eq!(rows.slice(PAGE * 2 + 2, 10).len(), 1);
        assert!(rows.slice(PAGE * 3, 10).is_empty());
    }

    #[test]
    fn values_read_as_declared() {
        let t = NaiveDate::from_ymd_opt(2026, 1, 31).unwrap().and_hms_micro_opt(13, 5, 9, 250_000).unwrap();
        let d = |v: Value, k: ColumnKind| display(&v, k).unwrap();
        assert_eq!(d(Value::Decimal(Decimal::new(-1250, 2)), ColumnKind::Decimal), "-12.50");
        assert_eq!(d(Value::Timestamp(t), ColumnKind::Timestamp), "2026-01-31 13:05:09.25");
        assert_eq!(d(Value::Timestamp(t), ColumnKind::Date), "2026-01-31");
        assert_eq!(d(Value::Timestamp(t), ColumnKind::Time), "13:05:09.25");
        assert_eq!(d(Value::Binary(vec![0xde, 0xad]), ColumnKind::Blob), "0xDEAD");
        assert_eq!(d(Value::Boolean(true), ColumnKind::Boolean), "TRUE");
        assert_eq!(display(&Value::Null, ColumnKind::Text), None);
        let long = d(Value::Text("é".repeat(MAX_CELL)), ColumnKind::TextBlob);
        assert!(long.len() <= MAX_CELL + "…".len() && long.ends_with('…'));
    }
}
