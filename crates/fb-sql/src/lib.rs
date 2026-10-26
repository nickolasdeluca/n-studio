//! Firebird SQL text, as the editor and the script runner need it: where
//! each statement starts and ends.
//!
//! Statements end at a terminator outside strings, quoted names and
//! comments; `SET TERM` changes the terminator, which is how a script holds
//! PSQL bodies full of `;`. Ranges are byte offsets into the text.

pub mod lexer;

use std::ops::Range;

use lexer::{tokenize, Token, TokenKind};
use serde::Serialize;

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
    let tokens = tokenize(text);
    // Byte by byte, whether it is code: only code can hold a terminator
    let mut code = vec![false; text.len()];
    for t in &tokens {
        if t.kind.is_code() {
            code[t.range.clone()].fill(true);
        }
    }

    let mut statements = Vec::new();
    let mut terminator = ";".to_string();
    let mut start = 0;
    let mut at = 0;
    while at < text.len() {
        if !text.is_char_boundary(at) || !code[at] || !text[at..].starts_with(&terminator) {
            at += 1;
            continue;
        }
        let end = at + terminator.len();
        if !code[at..end].iter().all(|&c| c) {
            at += 1;
            continue;
        }
        if let Some(statement) = statement(text, &tokens, start..at, end) {
            if let StatementKind::SetTerm { terminator: next } = &statement.kind {
                terminator = next.clone();
            }
            statements.push(statement);
        }
        start = end;
        at = end;
    }
    if let Some(last) = statement(text, &tokens, start..text.len(), text.len()) {
        statements.push(last);
    }
    statements
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
    let kind = classify(text, &range, &inside, &words);
    let unterminated_literal = tokens.iter().any(|t| t.unterminated && t.range.start < range.end && t.range.end > range.start);
    Some(Statement { range, span_end, kind, unterminated_literal })
}

fn classify(text: &str, range: &Range<usize>, tokens: &[&Token], words: &[String]) -> StatementKind {
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
                "KEEP_TRAN_PARAMS", "PER_TABLE_STATS", "EXEC_PATH_DISPLAY", "CONNECT",
            ];
            // SET TIME is isql's; SET TIME ZONE is SQL (Firebird 4)
            if ISQL.contains(&w) || (w == "TIME" && next != Some("ZONE")) {
                StatementKind::Isql
            } else {
                StatementKind::Sql
            }
        }
        (Some("SHOW" | "INPUT" | "OUTPUT" | "QUIT" | "EXIT" | "SHELL" | "EDIT" | "HELP"), _, _) if leading(1) => {
            StatementKind::Isql
        }
        _ => StatementKind::Sql,
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
