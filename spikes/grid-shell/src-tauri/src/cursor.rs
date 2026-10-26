//! Forward-only cursors with a scroll-back store.
//!
//! A cursor fetches from its source only as far as the grid has scrolled, in
//! pages, and keeps every row fetched so far so scrolling back costs no round
//! trip — the IBExpert model. What that store costs in memory at 1M rows is one
//! of the things this spike measures.
//!
//!
//! Storage and synthetic data live in `spikes/rowstore`, shared with grid-slint.

use serde::Serialize;
use std::collections::HashMap;

use crate::db::DbCursor;

pub use crate::rowstore::{synthetic_columns, Cell, PAGE};
use crate::rowstore::{synthetic_row, Page};

pub enum Source {
    Synthetic { total: usize, cols: usize },
    Db(DbCursor),
}

pub struct Cursor {
    pub columns: Vec<String>,
    source: Source,
    pages: Vec<Page>,
    /// Rows stored across `pages`.
    len: usize,
    done: bool,
}

#[derive(Serialize)]
pub struct CursorInfo {
    pub id: u64,
    pub columns: Vec<String>,
}

#[derive(Serialize)]
pub struct Chunk {
    pub start: usize,
    pub rows: Vec<Vec<Option<String>>>,
    /// Rows fetched from the source so far.
    pub fetched: usize,
    /// The source is exhausted: `fetched` is the final row count.
    pub done: bool,
}

impl Cursor {
    pub fn new(columns: Vec<String>, source: Source) -> Self {
        Self { columns, source, pages: Vec::new(), len: 0, done: false }
    }

    /// Rows `start..start + count`, fetching forward from the source until
    /// they exist or the source runs out.
    pub fn rows(&mut self, start: usize, count: usize) -> Result<Chunk, String> {
        let want = start + count;
        while self.len < want && !self.done {
            let rows = self.fetch_page()?;
            if rows.len() < PAGE {
                self.done = true;
            }
            self.len += rows.len();
            if !rows.is_empty() {
                self.pages.push(Page::pack(rows, self.columns.len()));
            }
        }

        // Every page but the last holds exactly PAGE rows.
        let end = want.min(self.len);
        let start = start.min(end);
        let rows = (start..end).map(|i| self.pages[i / PAGE].row(i % PAGE)).collect();

        Ok(Chunk { start, rows, fetched: self.len, done: self.done })
    }

    fn fetch_page(&mut self) -> Result<Vec<Box<[Cell]>>, String> {
        match &mut self.source {
            Source::Synthetic { total, cols } => {
                let from = self.len;
                let to = (from + PAGE).min(*total);
                Ok((from..to).map(|i| synthetic_row(i, *cols)).collect())
            }
            Source::Db(db) => db.fetch(PAGE),
        }
    }

    pub fn db(&mut self) -> Option<&mut DbCursor> {
        match &mut self.source {
            Source::Db(db) => Some(db),
            Source::Synthetic { .. } => None,
        }
    }
}

#[derive(Default)]
pub struct Registry {
    next_id: u64,
    cursors: HashMap<u64, Cursor>,
}

impl Registry {
    pub fn insert(&mut self, cursor: Cursor) -> CursorInfo {
        self.next_id += 1;
        let info = CursorInfo { id: self.next_id, columns: cursor.columns.clone() };
        self.cursors.insert(self.next_id, cursor);
        info
    }

    pub fn get(&mut self, id: u64) -> Result<&mut Cursor, String> {
        self.cursors.get_mut(&id).ok_or_else(|| format!("no cursor {id}"))
    }

    pub fn close(&mut self, id: u64) {
        self.cursors.remove(&id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_span_pages_and_stop_at_the_end() {
        let total = PAGE * 2 + 7;
        let mut c = Cursor::new(synthetic_columns(6), Source::Synthetic { total, cols: 6 });
        let chunk = c.rows(PAGE - 2, 4).unwrap();
        assert_eq!(chunk.rows.len(), 4);
        assert_eq!(chunk.rows[2][0].as_deref(), Some(&*(PAGE + 1).to_string()));
        let tail = c.rows(total - 3, 50).unwrap();
        assert_eq!(tail.rows.len(), 3);
        assert!(tail.done);
        assert_eq!(tail.fetched, total);
    }
}
