//! The scroll-back row store and the synthetic data, shared by the grid spikes
//! (`grid-shell`, Tauri; `grid-slint`, Slint) so they measure the same thing.
//! Included by path rather than a crate: the spikes are throwaway.
//!
//! Rows are stored a page at a time in one text buffer plus an offset per
//! cell. A heap allocation per cell cost ~45 bytes of overhead each: 1.3 GB
//! for 1M rows x 30 columns.

#![allow(dead_code)]

/// Rows per page. Matches the driver's op_fetch batch.
pub const PAGE: usize = 200;

/// A cell as the grid shows it: display text, or null.
pub type Cell = Option<Box<str>>;

/// Up to `PAGE` rows: every cell's text back to back in `text`, and the end
/// offset of each cell in `ends` (row-major). A null takes no text and stores
/// the previous cell's end with `NULL_BIT` set, so every cell's start is the
/// previous entry's end: O(1) access.
pub struct Page {
    text: String,
    ends: Vec<u32>,
    pub rows: usize,
    cols: usize,
}

const NULL_BIT: u32 = 1 << 31;

impl Page {
    pub fn pack(rows: Vec<Box<[Cell]>>, cols: usize) -> Self {
        let cells = rows.len() * cols;
        let bytes: usize = rows.iter().flat_map(|r| r.iter()).flatten().map(|c| c.len()).sum();
        assert!(bytes < NULL_BIT as usize, "page text past 2 GiB");

        let mut page = Page { text: String::with_capacity(bytes), ends: Vec::with_capacity(cells), rows: rows.len(), cols };
        for cell in rows.iter().flat_map(|r| r.iter()) {
            match cell {
                Some(c) => {
                    page.text.push_str(c);
                    page.ends.push(page.text.len() as u32);
                }
                None => page.ends.push(page.text.len() as u32 | NULL_BIT),
            }
        }
        page
    }

    /// Cell `c` of row `r` in this page.
    pub fn cell(&self, r: usize, c: usize) -> Option<&str> {
        let i = r * self.cols + c;
        let end = self.ends[i];
        if end & NULL_BIT != 0 {
            return None;
        }
        let start = if i == 0 { 0 } else { self.ends[i - 1] & !NULL_BIT };
        Some(&self.text[start as usize..end as usize])
    }

    pub fn row(&self, r: usize) -> Vec<Option<String>> {
        (0..self.cols).map(|c| self.cell(r, c).map(str::to_owned)).collect()
    }
}

/// Column names for a synthetic result: a realistic mix repeated to `cols`.
pub fn synthetic_columns(cols: usize) -> Vec<String> {
    const BASE: [&str; 6] = ["ID", "NOME", "PRECO", "ESTOQUE", "ALTERADO_EM", "OBS"];
    (0..cols)
        .map(|c| match c / BASE.len() {
            0 => BASE[c].to_string(),
            n => format!("{}_{}", BASE[c % BASE.len()], n),
        })
        .collect()
}

/// A deterministic row shaped like ERP data: ids, accented text, scaled
/// numerics rendered from integers (never through f64), timestamps, nulls.
pub fn synthetic_row(i: usize, cols: usize) -> Box<[Cell]> {
    (0..cols)
        .map(|c| {
            let n = i.wrapping_mul(2654435761).wrapping_add(c * 40503) % 1_000_000;
            let cell = match c % 6 {
                0 => (i + 1).to_string(),
                1 => format!("Produto ação nº {n} — preço promocional"),
                2 => format!("{}.{:02}", n / 100, n % 100),
                3 => (n % 5000).to_string(),
                4 => format!(
                    "2026-{:02}-{:02} {:02}:{:02}:{:02}",
                    n % 12 + 1,
                    n % 28 + 1,
                    n % 24,
                    n % 60,
                    (n / 60) % 60
                ),
                _ if n % 7 == 0 => return None,
                _ => format!("Observação {n}"),
            };
            Some(cell.into_boxed_str())
        })
        .collect()
}

/// `total` synthetic rows, packed: what a fully fetched result looks like.
pub fn synthetic_pages(total: usize, cols: usize) -> Vec<Page> {
    (0..total)
        .step_by(PAGE)
        .map(|from| Page::pack((from..(from + PAGE).min(total)).map(|i| synthetic_row(i, cols)).collect(), cols))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell(s: &str) -> Cell {
        Some(s.into())
    }

    #[test]
    fn pages_round_trip_cells_and_nulls() {
        let rows: Vec<Box<[Cell]>> = vec![
            vec![cell("1"), None, cell("ação")].into(),
            vec![None, None, cell("")].into(),
            vec![cell("3"), cell("x"), None].into(),
        ];
        let page = Page::pack(rows, 3);
        assert_eq!(page.row(0), vec![Some("1".into()), None, Some("ação".into())]);
        assert_eq!(page.row(1), vec![None, None, Some("".into())]);
        assert_eq!(page.row(2), vec![Some("3".into()), Some("x".into()), None]);
        assert_eq!(page.cell(2, 1), Some("x"));
    }

    #[test]
    fn synthetic_pages_hold_every_row() {
        let pages = synthetic_pages(PAGE * 2 + 7, 6);
        assert_eq!(pages.len(), 3);
        assert_eq!(pages[2].rows, 7);
        assert_eq!(pages[1].cell(1, 0), Some(&*(PAGE + 2).to_string()));
    }
}
