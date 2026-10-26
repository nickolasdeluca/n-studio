//! A result's rows written out as delimited text: CSV, or TSV with a tab.
//! Fields are quoted as RFC 4180 has it, only when they need to be; a null
//! is an empty field, lines end in CRLF.

use std::io::{self, Write};

use serde::Deserialize;

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportOptions {
    /// `,`, `;` (what a spreadsheet in a comma-decimal locale expects) or a tab
    pub delimiter: char,
    /// A first line with the column names
    pub header: bool,
    /// A UTF-8 byte order mark, without which Excel reads the file in the
    /// system's ANSI code page
    pub bom: bool,
}

pub struct Writer<W: Write> {
    out: W,
    delimiter: char,
}

impl<W: Write> Writer<W> {
    pub fn new(mut out: W, options: ExportOptions, names: &[String]) -> io::Result<Self> {
        if options.bom {
            out.write_all("\u{feff}".as_bytes())?;
        }
        let mut writer = Writer { out, delimiter: options.delimiter };
        if options.header {
            writer.row(names.iter().map(|n| Some(n.as_str())))?;
        }
        Ok(writer)
    }

    pub fn row<'a>(&mut self, cells: impl Iterator<Item = Option<&'a str>>) -> io::Result<()> {
        for (i, cell) in cells.enumerate() {
            if i > 0 {
                write!(self.out, "{}", self.delimiter)?;
            }
            if let Some(cell) = cell {
                self.field(cell)?;
            }
        }
        self.out.write_all(b"\r\n")
    }

    fn field(&mut self, s: &str) -> io::Result<()> {
        let quoted = s.contains([self.delimiter, '"', '\r', '\n']);
        if !quoted {
            return self.out.write_all(s.as_bytes());
        }
        self.out.write_all(b"\"")?;
        self.out.write_all(s.replace('"', "\"\"").as_bytes())?;
        self.out.write_all(b"\"")
    }

    pub fn finish(mut self) -> io::Result<()> {
        self.out.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn export(options: ExportOptions, rows: &[Vec<Option<&str>>]) -> String {
        let mut out = Vec::new();
        let mut w = Writer::new(&mut out, options, &["ID".into(), "NAME".into()]).unwrap();
        for row in rows {
            w.row(row.iter().copied()).unwrap();
        }
        w.finish().unwrap();
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn quotes_only_what_needs_it() {
        let rows = [vec![Some("1"), Some("plain")], vec![Some("2"), Some("a, \"b\"\nc")], vec![None, Some("")]];
        let csv = export(ExportOptions { delimiter: ',', header: true, bom: false }, &rows);
        assert_eq!(csv, "ID,NAME\r\n1,plain\r\n2,\"a, \"\"b\"\"\nc\"\r\n,\r\n");
        let tsv = export(ExportOptions { delimiter: '\t', header: false, bom: true }, &rows[..2]);
        assert_eq!(tsv, "\u{feff}1\tplain\r\n2\t\"a, \"\"b\"\"\nc\"\r\n");
    }
}
