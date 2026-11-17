//! Firebird SQL text, as the editor and the script runner need it: where
//! each statement starts and ends.
//!
//! Statements end at a terminator outside strings, quoted names and
//! comments; `SET TERM` changes the terminator, which is how a script holds
//! PSQL bodies full of `;`. Ranges are byte offsets into the text.

pub mod format;
pub mod isql;
pub mod lexer;
pub mod psql;
mod subject;

use std::collections::VecDeque;
use std::ops::Range;

use lexer::{tokenize, Token, TokenKind, Tokens};
use serde::Serialize;
pub use subject::{classify, ObjectKind, Operation, Subject};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Statement {
    /// From the first to the last character of code, comments around it and
    /// the terminator left out
    pub range: Range<usize>,
    /// Up to the end of its terminator, or `range.end` without one
    pub span_end: usize,
    pub kind: StatementKind,
    /// Ends inside a string, a quoted name or a comment
    pub unterminated_literal: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StatementKind {
    /// For the server
    Sql,
    /// `SET TERM <terminator>`: handled here, never sent
    SetTerm { terminator: String },
    /// An isql command (`SET SQL DIALECT`, `SHOW TABLE`, …): not SQL, and no
    /// server would take it
    Isql,
}

/// Splits a script into statements, `;`-terminated until a `SET TERM`.
pub fn split(text: &str) -> Vec<Statement> {
    Splitter::new(text).collect()
}

/// A script's statements one at a time, as [`split`] gives them, holding
/// only the tokens of the statement being read: a script of hundreds of
/// megabytes costs no more than its text.
pub struct Splitter<'a> {
    text: &'a str,
    tokens: Tokens<'a>,
    terminator: String,
    /// Where the statement being read starts
    start: usize,
    /// Its tokens so far, in text order
    pending: Vec<Token>,
    /// The run of code the last code token is in: only code can hold a
    /// terminator. Scanned for one from `scan` on, as far as it goes.
    run_end: usize,
    scan: usize,
    found: VecDeque<Statement>,
    done: bool,
}

impl<'a> Splitter<'a> {
    pub fn new(text: &'a str) -> Self {
        Self {
            text,
            tokens: Tokens::new(text),
            terminator: ";".into(),
            start: 0,
            pending: Vec::new(),
            run_end: usize::MAX,
            scan: 0,
            found: VecDeque::new(),
            done: false,
        }
    }

    /// Looks for the terminator in the run of code, as far as it is known
    fn scan_run(&mut self) {
        let text = self.text;
        while self.scan + self.terminator.len() <= self.run_end {
            let at = self.scan;
            if !text.is_char_boundary(at) || !text[at..].starts_with(self.terminator.as_str()) {
                self.scan += 1;
                continue;
            }
            let end = at + self.terminator.len();
            if let Some(statement) = statement(text, &self.pending, self.start..at, end) {
                if let StatementKind::SetTerm { terminator } = &statement.kind {
                    self.terminator = terminator.clone();
                }
                self.found.push_back(statement);
            }
            self.start = end;
            let start = end;
            self.pending.retain(|t| t.range.start >= start);
            self.scan = end;
        }
    }
}

impl Iterator for Splitter<'_> {
    type Item = Statement;

    fn next(&mut self) -> Option<Statement> {
        loop {
            if let Some(statement) = self.found.pop_front() {
                return Some(statement);
            }
            if self.done {
                return None;
            }
            match self.tokens.next() {
                Some(token) if token.kind.is_code() => {
                    // A new run of code when something that is not code
                    // came between
                    if token.range.start != self.run_end {
                        self.scan = token.range.start;
                    }
                    self.run_end = token.range.end;
                    self.pending.push(token);
                    self.scan_run();
                }
                Some(token) => self.pending.push(token),
                None => {
                    self.done = true;
                    let len = self.text.len();
                    if let Some(last) = statement(self.text, &self.pending, self.start..len, len) {
                        return Some(last);
                    }
                }
            }
        }
    }
}

/// The statement the cursor is in or, between statements, the one before
/// it, as an editor's "execute current statement" wants
pub fn statement_at(statements: &[Statement], offset: usize) -> Option<&Statement> {
    statements
        .iter()
        .rev()
        .find(|s| s.range.start <= offset)
        .or_else(|| statements.first())
}

/// The statement in `range`, or `None` if it holds no code
fn statement(text: &str, tokens: &[Token], range: Range<usize>, span_end: usize) -> Option<Statement> {
    // Tokens are in text order: find the statement's slice, not scan them all
    let from = tokens.partition_point(|t| t.range.start < range.start);
    let to = tokens.partition_point(|t| t.range.end <= range.end);
    let tokens = &tokens[from..to.max(from)];
    let inside: Vec<&Token> = tokens.iter().filter(|t| !t.kind.is_trivia()).collect();
    let first = inside.first()?;
    let last = inside.last()?;
    let range = first.range.start..last.range.end;
    let words: Vec<String> = inside
        .iter()
        .filter(|t| t.kind == TokenKind::Word)
        .take(3)
        .map(|t| text[t.range.clone()].to_ascii_uppercase())
        .collect();
    let kind = kind_of(text, &range, &inside, &words);
    let unterminated_literal = tokens.iter().any(|t| t.unterminated && t.range.start < range.end && t.range.end > range.start);
    Some(Statement { range, span_end, kind, unterminated_literal })
}

fn kind_of(text: &str, range: &Range<usize>, tokens: &[&Token], words: &[String]) -> StatementKind {
    let word = |i: usize| words.get(i).map(String::as_str);
    // The leading words must be the statement's first tokens
    let leading = |n: usize| tokens.iter().take(n).all(|t| t.kind == TokenKind::Word);

    if word(0) == Some("SET") && word(1) == Some("TERM") && leading(2) {
        let after_term = tokens[1].range.end;
        let terminator = text[after_term..range.end].trim();
        if !terminator.is_empty() && !terminator.contains(char::is_whitespace) {
            return StatementKind::SetTerm { terminator: terminator.to_string() };
        }
    }
    match (word(0), word(1), word(2)) {
        (Some("SET"), Some(w), next) if leading(2) => {
            const ISQL: &[&str] = &[
                "SQL", "AUTODDL", "AUTO", "NAMES", "PLAN", "PLANONLY", "EXPLAIN", "STATS", "COUNT", "ECHO", "LIST",
                "HEADING", "BAIL", "WARNINGS", "WNG", "BLOB", "BLOBDISPLAY", "ROWCOUNT", "MAXROWS", "WIDTH",
                "KEEP_TRAN_PARAMS", "PER_TABLE_STATS", "EXEC_PATH_DISPLAY", "CONNECT", "CLIENTLIB",
            ];
            // SET TIME is isql's; SET TIME ZONE is SQL (Firebird 4)
            if ISQL.contains(&w) || (w == "TIME" && next != Some("ZONE")) {
                StatementKind::Isql
            } else {
                StatementKind::Sql
            }
        }
        // CONNECT is isql's; DESCRIBE is IBExpert's, and means a COMMENT ON
        (Some("SHOW" | "INPUT" | "OUTPUT" | "QUIT" | "EXIT" | "SHELL" | "EDIT" | "HELP" | "CONNECT" | "DESCRIBE"), _, _)
            if leading(1) =>
        {
            StatementKind::Isql
        }
        _ => StatementKind::Sql,
    }
}

/// A statement's parameters, ready to bind: `sql` with every `:name`
/// replaced by `?`, and each position's name (`None` for a `?`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Parameters {
    pub sql: String,
    pub names: Vec<Option<String>>,
}

/// Finds a statement's parameters. Firebird takes only `?`; `:name` is the
/// editor's, bound by position like IBExpert does. In PSQL (`EXECUTE BLOCK`,
/// `CREATE PROCEDURE` and the like) `:name` is a variable and stays, and only
/// `?` counts: `EXECUTE BLOCK (x INT = ?)` takes one.
pub fn parameters(statement: &str) -> Parameters {
    let tokens = tokenize(statement);
    let psql = is_psql(statement, &tokens);
    let mut sql = String::with_capacity(statement.len());
    let mut names = Vec::new();
    let mut at = 0;
    for t in tokens.iter().filter(|t| t.kind == TokenKind::Param) {
        let text = &statement[t.range.clone()];
        if text == "?" {
            names.push(None);
        } else if !psql {
            sql.push_str(&statement[at..t.range.start]);
            sql.push('?');
            at = t.range.end;
            names.push(Some(text[1..].to_ascii_uppercase()));
        }
    }
    sql.push_str(&statement[at..]);
    Parameters { sql, names }
}

/// Whether a statement holds PSQL, where `:name` is a variable
fn is_psql(text: &str, tokens: &[Token]) -> bool {
    let words: Vec<String> = tokens
        .iter()
        .filter(|t| !t.kind.is_trivia())
        .take(5)
        .take_while(|t| t.kind == TokenKind::Word)
        .map(|t| text[t.range.clone()].to_ascii_uppercase())
        .collect();
    let w: Vec<&str> = words.iter().map(String::as_str).collect();
    const BODIES: &[&str] = &["PROCEDURE", "TRIGGER", "FUNCTION", "PACKAGE"];
    match w.as_slice() {
        ["EXECUTE", "BLOCK", ..] => true,
        ["CREATE", "OR", "ALTER", kind, ..] => BODIES.contains(kind),
        ["CREATE" | "ALTER" | "RECREATE", kind, ..] => BODIES.contains(kind),
        _ => false,
    }
}

/// Converts a byte offset into `text` to a UTF-16 offset, which is how an
/// editor in a web view counts
pub fn utf16_offset(text: &str, byte: usize) -> usize {
    text[..byte.min(text.len())].encode_utf16().count()
}

/// Converts a UTF-16 offset into `text` to a byte offset
pub fn byte_offset(text: &str, utf16: usize) -> usize {
    let mut units = 0;
    for (i, c) in text.char_indices() {
        if units >= utf16 {
            return i;
        }
        units += c.len_utf16();
    }
    text.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(script: &str) -> Vec<&str> {
        split(script).into_iter().map(|s| &script[s.range]).collect()
    }

    #[test]
    fn splits_on_terminators_outside_literals() {
        let script = "select 'a;b' from t; -- c;\n/* d; */ update t set x = \"e;\";\n\nselect 1 from rdb$database";
        assert_eq!(
            texts(script),
            vec!["select 'a;b' from t", "update t set x = \"e;\"", "select 1 from rdb$database"]
        );
    }

    #[test]
    fn set_term_holds_psql_bodies_together() {
        let script = "\
SET TERM ^ ;
CREATE PROCEDURE P AS
BEGIN
  INSERT INTO T VALUES (1);
  SUSPEND;
END^
SET TERM ; ^
COMMIT;";
        let statements = split(script);
        let kinds: Vec<_> = statements.iter().map(|s| s.kind.clone()).collect();
        assert_eq!(
            kinds,
            vec![
                StatementKind::SetTerm { terminator: "^".into() },
                StatementKind::Sql,
                StatementKind::SetTerm { terminator: ";".into() },
                StatementKind::Sql,
            ]
        );
        assert!(script[statements[1].range.clone()].ends_with("SUSPEND;\nEND"));
        assert_eq!(&script[statements[3].range.clone()], "COMMIT");
    }

    #[test]
    fn multi_character_terminators() {
        let script = "set term !! ;\nexecute block as begin end!!\nset term ;!!\nselect 1 from rdb$database;";
        assert_eq!(
            texts(script),
            vec!["set term !!", "execute block as begin end", "set term ;", "select 1 from rdb$database"]
        );
    }

    #[test]
    fn empty_statements_and_trailing_comments_vanish() {
        assert_eq!(texts(";;  ; -- nothing\n"), Vec::<&str>::new());
        assert_eq!(texts("select 1 from rdb$database; -- done"), vec!["select 1 from rdb$database"]);
    }

    #[test]
    fn tells_isql_commands_apart() {
        let kinds = |s: &str| split(s).into_iter().map(|s| s.kind).collect::<Vec<_>>();
        assert_eq!(kinds("SET SQL DIALECT 3"), vec![StatementKind::Isql]);
        assert_eq!(kinds("show tables"), vec![StatementKind::Isql]);
        assert_eq!(kinds("SET CLIENTLIB 'fbclient.dll'"), vec![StatementKind::Isql]);
        assert_eq!(kinds("CONNECT 'db' USER 'u'"), vec![StatementKind::Isql]);
        assert_eq!(kinds("DESCRIBE TABLE T 'text'"), vec![StatementKind::Isql]);
        assert_eq!(kinds("SET TIME ZONE 'America/Sao_Paulo'"), vec![StatementKind::Sql]);
        assert_eq!(kinds("set generator g to 5"), vec![StatementKind::Sql]);
        assert_eq!(kinds("set transaction read committed"), vec![StatementKind::Sql]);
    }

    #[test]
    fn statement_at_the_cursor() {
        let script = "select 1 from rdb$database;\n\nselect 2 from rdb$database;";
        let s = split(script);
        assert_eq!(statement_at(&s, 3).unwrap().range.start, 0);
        // Right after the first terminator: still the first statement
        assert_eq!(statement_at(&s, 27).unwrap().range.start, 0);
        assert_eq!(statement_at(&s, 30).unwrap().range.start, 29);
        assert_eq!(statement_at(&s, script.len()).unwrap().range.start, 29);
    }

    #[test]
    fn utf16_offsets_round_trip() {
        let text = "select 'ação 😀' from t";
        let byte = text.find("from").unwrap();
        let units = utf16_offset(text, byte);
        assert_eq!(units, "select 'ação 😀' ".encode_utf16().count());
        assert_eq!(byte_offset(text, units), byte);
    }

    #[test]
    fn named_parameters_become_positional() {
        let p = parameters("select * from t where a = :id and b = ? and c = :Id and d = ':no'");
        assert_eq!(p.sql, "select * from t where a = ? and b = ? and c = ? and d = ':no'");
        assert_eq!(p.names, vec![Some("ID".into()), None, Some("ID".into())]);
    }

    #[test]
    fn psql_variables_are_not_parameters() {
        let block = "execute block (x int = ?) returns (y int) as begin y = :x; suspend; end";
        let p = parameters(block);
        assert_eq!(p.sql, block);
        assert_eq!(p.names, vec![None]);
        let proc = "create or alter procedure p (a int) as declare v int; begin v = :a; end";
        assert_eq!(parameters(proc).names, Vec::<Option<String>>::new());
        assert_eq!(parameters(proc).sql, proc);
    }

    #[test]
    fn flags_an_unclosed_string() {
        assert!(split("select 'abc from t")[0].unterminated_literal);
        assert!(!split("select 'abc' from t")[0].unterminated_literal);
    }
}

#[cfg(test)]
mod scale {
    use super::*;

    #[test]
    fn a_large_script_splits_in_linear_time() {
        let script = "insert into t (a, b) values (1, 'x;y'); -- row\n".repeat(50_000);
        let started = std::time::Instant::now();
        assert_eq!(split(&script).len(), 50_000);
        // Quadratic took minutes here; linear is well under a second, even unoptimized
        assert!(started.elapsed().as_secs() < 5, "{:?}", started.elapsed());
    }
}
