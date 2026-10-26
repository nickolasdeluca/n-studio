//! A query's rows as the grid shows them, kept here rather than in the UI:
//! the grid asks for the rows in view, and only those cross the bridge.
//!
//! Rows are stored as display text, a page at a time in one buffer plus an
//! offset per cell: a heap allocation per cell cost ~45 bytes of overhead
//! each, 1.3 GB for 1M rows x 30 columns (see docs/M0-FINDINGS.md).

use std::cmp::Ordering;

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
    /// For each blob column, its blobs' ids by row (0 for null): the cell
    /// shows a marker, the content is read when asked for
    blobs: Vec<Option<Vec<u64>>>,
    /// The rows in a sorted order, by their place in fetch order
    order: Option<Vec<u32>>,
}

/// How the grid has the rows sorted
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Sort {
    pub column: usize,
    pub descending: bool,
}

impl Rows {
    pub fn new(kinds: Vec<ColumnKind>) -> Self {
        let blobs = kinds.iter().map(|k| matches!(k, ColumnKind::Blob | ColumnKind::TextBlob).then(Vec::new)).collect();
        Self { kinds, pages: Vec::new(), len: 0, blobs, order: None }
    }

    /// Row `row` as shown: its place in fetch order
    fn physical(&self, row: usize) -> usize {
        self.order.as_ref().map_or(row, |o| o[row] as usize)
    }

    fn cell(&self, row: usize, col: usize) -> Option<&str> {
        self.pages[row / PAGE].cell((row % PAGE) * self.kinds.len() + col)
    }

    /// Sorts the rows by a column, as its kind compares: numbers by value,
    /// text ignoring case, nulls first. Ties keep fetch order. `None` goes
    /// back to fetch order. Blob columns hold markers, not their content.
    pub fn sort(&mut self, sort: Option<Sort>) {
        let Some(Sort { column, descending }) = sort else {
            self.order = None;
            return;
        };
        let kind = self.kinds[column];
        let mut order: Vec<u32> = (0..self.len as u32).collect();
        order.sort_by(|&a, &b| {
            let by = compare(kind, self.cell(a as usize, column), self.cell(b as usize, column));
            if descending {
                by.reverse()
            } else {
                by
            }
        });
        self.order = Some(order);
    }

    /// The blob in row `row` of column `col`: its id and whether it is text.
    /// `None` for a null, or a column without blobs.
    pub fn blob(&self, row: usize, col: usize) -> Option<(u64, bool)> {
        if row >= self.len {
            return None;
        }
        let id = *self.blobs.get(col)?.as_ref()?.get(self.physical(row))?;
        (id != 0).then(|| (id, self.kinds[col] == ColumnKind::TextBlob))
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn push(&mut self, row: Vec<Value>) {
        if self.pages.last().is_none_or(|p| p.rows == PAGE) {
            self.pages.push(Page::new(self.kinds.len()));
        }
        for (ids, v) in self.blobs.iter_mut().zip(&row) {
            if let Some(ids) = ids {
                ids.push(if let Value::BlobRef { id, .. } = v { *id } else { 0 });
            }
        }
        let kinds = &self.kinds;
        let page = self.pages.last_mut().expect("pushed above");
        page.push(row.iter().zip(kinds).map(|(v, k)| display(v, *k)));
        // Rows fetched after a sort come after the sorted ones
        if let Some(order) = &mut self.order {
            order.push(self.len as u32);
        }
        self.len += 1;
    }

    /// Rows `from..from + count` as shown, as many as there are
    pub fn slice(&self, from: usize, count: usize) -> Vec<Vec<Cell>> {
        (from..(from.saturating_add(count)).min(self.len))
            .map(|r| self.row(r).map(|c| c.map(str::to_owned)).collect())
            .collect()
    }

    /// Row `row` as shown
    pub fn row(&self, row: usize) -> impl Iterator<Item = Option<&str>> {
        let r = self.physical(row);
        (0..self.kinds.len()).map(move |c| self.cell(r, c))
    }

    pub fn kinds(&self) -> &[ColumnKind] {
        &self.kinds
    }
}

/// Two cells of a column, by its kind; null first
fn compare(kind: ColumnKind, a: Option<&str>, b: Option<&str>) -> Ordering {
    let (a, b) = match (a, b) {
        (Some(a), Some(b)) => (a, b),
        (a, b) => return a.is_some().cmp(&b.is_some()),
    };
    match kind {
        ColumnKind::Integer | ColumnKind::Decimal => compare_decimal(a, b),
        ColumnKind::Float => {
            let f = |s: &str| s.parse::<f64>().unwrap_or(f64::NAN);
            f(a).total_cmp(&f(b))
        }
        // Dates and times read in ISO order; other kinds as text, case aside
        _ => {
            let fold = |s: &str| s.chars().flat_map(char::to_lowercase).collect::<Vec<_>>();
            let by = if a.is_ascii() && b.is_ascii() {
                a.bytes().map(|c| c.to_ascii_lowercase()).cmp(b.bytes().map(|c| c.to_ascii_lowercase()))
            } else {
                fold(a).cmp(&fold(b))
            };
            by.then_with(|| a.cmp(b))
        }
    }
}

/// Two exact numbers as `display` writes them (`-12.50`, any length), by
/// value: an INT128 or a NUMERIC(38) would not fit a float
fn compare_decimal(a: &str, b: &str) -> Ordering {
    fn parts(s: &str) -> (bool, &str, &str) {
        let (negative, s) = s.strip_prefix('-').map_or((false, s), |s| (true, s));
        let (int, frac) = s.split_once('.').unwrap_or((s, ""));
        let int = int.trim_start_matches('0');
        let frac = frac.trim_end_matches('0');
        // -0 is 0
        (negative && !(int.is_empty() && frac.is_empty()), int, frac)
    }
    let (na, ia, fa) = parts(a);
    let (nb, ib, fb) = parts(b);
    if na != nb {
        return nb.cmp(&na);
    }
    let magnitude = ia.len().cmp(&ib.len()).then_with(|| ia.cmp(ib)).then_with(|| fa.cmp(fb));
    if na {
        magnitude.reverse()
    } else {
        magnitude
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
        // Not read yet: the grid marks it, a viewer reads it
        Value::BlobRef { text: true, .. } => "(MEMO)".to_string(),
        Value::BlobRef { text: false, .. } => "(BLOB)".to_string(),
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

    #[test]
    fn sorts_by_value_with_nulls_first() {
        let mut rows = Rows::new(vec![ColumnKind::Decimal, ColumnKind::Text, ColumnKind::TextBlob]);
        let numbers = ["10.5", "-2", "9.75", "-10.25", "0", "170141183460469231731687303715884105727"];
        for (i, n) in numbers.iter().enumerate() {
            let text = Value::Text(["b", "A", "a", "Ç", "c", "B"][i].into());
            rows.push(vec![Value::Text(n.to_string()), text, Value::BlobRef { id: i as u64 + 1, text: true }]);
        }
        rows.push(vec![Value::Null, Value::Null, Value::Null]);
        let column =
            |rows: &Rows, c: usize| (0..rows.len()).map(|r| rows.slice(r, 1)[0][c].clone()).collect::<Vec<_>>();
        let numbers = |rows: &Rows| column(rows, 0);

        rows.sort(Some(Sort { column: 0, descending: false }));
        let sorted = numbers(&rows);
        assert_eq!(sorted[0], None);
        let shown: Vec<_> = sorted[1..].iter().map(|c| c.clone().unwrap()).collect();
        let mut expected = shown.clone();
        expected.sort_by(|a, b| compare_decimal(a, b));
        assert_eq!(shown, expected);
        assert_eq!(compare_decimal("-10.25", "-2"), Ordering::Less);
        assert_eq!(compare_decimal("9.75", "10.5"), Ordering::Less);
        assert_eq!(compare_decimal("-0", "0.00"), Ordering::Equal);
        assert_eq!(compare_decimal("170141183460469231731687303715884105727", "9.5"), Ordering::Greater);
        // The blob ids follow their rows
        let first = rows.slice(1, 1)[0][1].clone();
        let (id, _) = rows.blob(1, 2).unwrap();
        assert_eq!(first, Some(["b", "A", "a", "Ç", "c", "B"][id as usize - 1].to_string()));

        rows.sort(Some(Sort { column: 1, descending: true }));
        assert_eq!(
            column(&rows, 1),
            ["Ç", "c", "b", "B", "a", "A"].map(|s| Some(s.to_string())).into_iter().chain([None]).collect::<Vec<_>>()
        );

        rows.sort(None);
        assert_eq!(rows.slice(0, 1)[0][1], Some("b".into()));
    }

    #[test]
    fn blob_columns_keep_their_ids() {
        let mut rows = Rows::new(vec![ColumnKind::Integer, ColumnKind::TextBlob, ColumnKind::Blob]);
        rows.push(vec![Value::Integer(1), Value::BlobRef { id: 42, text: true }, Value::Null]);
        rows.push(vec![Value::Integer(2), Value::Null, Value::BlobRef { id: 7, text: false }]);
        assert_eq!(rows.slice(0, 1)[0], vec![Some("1".into()), Some("(MEMO)".into()), None]);
        assert_eq!(rows.blob(0, 1), Some((42, true)));
        assert_eq!(rows.blob(1, 2), Some((7, false)));
        assert_eq!(rows.blob(1, 1), None);
        assert_eq!(rows.blob(0, 0), None);
    }
}
