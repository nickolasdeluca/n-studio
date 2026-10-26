//! Just enough lexing to know what is code and what is not: strings, quoted
//! identifiers and comments may hold anything, terminators included.

use std::ops::Range;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    Space,
    /// `-- …` to the end of the line
    LineComment,
    /// `/* … */`
    BlockComment,
    /// `'…'` with `''` inside, or `q'{…}'` (Firebird 3+)
    String,
    /// `"…"` with `""` inside: an identifier in dialect 3, a string in
    /// dialect 1. Either way its content is not code.
    Quoted,
    /// A keyword or an identifier: letters, digits, `_` and `$`
    Word,
    Number,
    /// `?` or `:name`
    Param,
    /// Any other character
    Punct,
}

impl TokenKind {
    /// Whether the token is code, as opposed to a comment, a literal or
    /// space: only code can end a statement.
    pub fn is_code(self) -> bool {
        matches!(self, TokenKind::Word | TokenKind::Number | TokenKind::Param | TokenKind::Punct)
    }

    pub fn is_trivia(self) -> bool {
        matches!(self, TokenKind::Space | TokenKind::LineComment | TokenKind::BlockComment)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    pub kind: TokenKind,
    /// Byte range in the text
    pub range: Range<usize>,
    /// A string, quoted identifier or comment the text ends inside of
    pub unterminated: bool,
}

pub fn tokenize(text: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let start = i;
        let rest = &text[i..];
        let c = rest.chars().next().expect("i is on a char boundary");
        let mut unterminated = false;
        let kind = if c.is_whitespace() {
            i += rest.find(|c: char| !c.is_whitespace()).unwrap_or(rest.len());
            TokenKind::Space
        } else if rest.starts_with("--") {
            i += rest.find('\n').unwrap_or(rest.len());
            TokenKind::LineComment
        } else if rest.starts_with("/*") {
            match rest[2..].find("*/") {
                Some(end) => i += 2 + end + 2,
                None => {
                    i = bytes.len();
                    unterminated = true;
                }
            }
            TokenKind::BlockComment
        } else if let Some(len) = q_string(rest) {
            match len {
                Some(len) => i += len,
                None => {
                    i = bytes.len();
                    unterminated = true;
                }
            }
            TokenKind::String
        } else if c == '\'' || c == '"' {
            match quoted(rest, c as u8) {
                Some(len) => i += len,
                None => {
                    i = bytes.len();
                    unterminated = true;
                }
            }
            if c == '\'' {
                TokenKind::String
            } else {
                TokenKind::Quoted
            }
        } else if c.is_alphabetic() || c == '_' {
            i += rest.find(|c: char| !(c.is_alphanumeric() || c == '_' || c == '$')).unwrap_or(rest.len());
            TokenKind::Word
        } else if c.is_ascii_digit() || (c == '.' && rest[1..].starts_with(|c: char| c.is_ascii_digit())) {
            i += rest.find(|c: char| !(c.is_ascii_alphanumeric() || c == '.')).unwrap_or(rest.len());
            TokenKind::Number
        } else if c == '?' {
            i += 1;
            TokenKind::Param
        } else if c == ':' && rest[1..].starts_with(|c: char| c.is_alphabetic() || c == '_') {
            i += 1 + rest[1..].find(|c: char| !(c.is_alphanumeric() || c == '_' || c == '$')).unwrap_or(rest.len() - 1);
            TokenKind::Param
        } else {
            i += c.len_utf8();
            TokenKind::Punct
        };
        tokens.push(Token { kind, range: start..i, unterminated });
    }
    tokens
}

/// The length of `'…'` or `"…"` at the start of `s`, where a doubled quote
/// stands for one; `None` if it never closes.
fn quoted(s: &str, quote: u8) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut i = 1;
    while i < bytes.len() {
        if bytes[i] == quote {
            if bytes.get(i + 1) == Some(&quote) {
                i += 2;
                continue;
            }
            return Some(i + 1);
        }
        i += 1;
    }
    None
}

/// A Firebird 3 alternative string literal: `q'<open>…<close>'`, where
/// `(`, `[`, `{` and `<` close with their pair and any other character with
/// itself. `None` if `s` does not start one; `Some(None)` if it never closes.
fn q_string(s: &str) -> Option<Option<usize>> {
    let mut chars = s.char_indices();
    let (_, q) = chars.next()?;
    if !q.eq_ignore_ascii_case(&'q') || chars.next()?.1 != '\'' {
        return None;
    }
    let (open_at, open) = chars.next()?;
    if open.is_whitespace() {
        return None;
    }
    let close = match open {
        '(' => ')',
        '[' => ']',
        '{' => '}',
        '<' => '>',
        c => c,
    };
    let body = open_at + open.len_utf8();
    let needle: String = [close, '\''].iter().collect();
    Some(s[body..].find(&needle).map(|end| body + end + needle.len()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use TokenKind::*;

    fn kinds(text: &str) -> Vec<(TokenKind, &str)> {
        tokenize(text).into_iter().map(|t| (t.kind, &text[t.range])).collect()
    }

    #[test]
    fn strings_comments_and_quoted_names_are_not_code() {
        assert_eq!(
            kinds("select 'a;b''c' from \"T;\" -- x;\n/* y; */ where id = :id_1"),
            vec![
                (Word, "select"),
                (Space, " "),
                (String, "'a;b''c'"),
                (Space, " "),
                (Word, "from"),
                (Space, " "),
                (Quoted, "\"T;\""),
                (Space, " "),
                (LineComment, "-- x;"),
                (Space, "\n"),
                (BlockComment, "/* y; */"),
                (Space, " "),
                (Word, "where"),
                (Space, " "),
                (Word, "id"),
                (Space, " "),
                (Punct, "="),
                (Space, " "),
                (Param, ":id_1"),
            ]
        );
    }

    #[test]
    fn q_strings_close_on_their_pair() {
        assert_eq!(kinds("q'{it's; }'"), vec![(String, "q'{it's; }'")]);
        assert_eq!(kinds("Q'!a'b!'"), vec![(String, "Q'!a'b!'")]);
        // Not a q-string: an identifier named q
        assert_eq!(kinds("q "), vec![(Word, "q"), (Space, " ")]);
    }

    #[test]
    fn unterminated_literals_run_to_the_end() {
        let t = tokenize("select 'abc; def");
        assert_eq!(t.last().unwrap().kind, String);
        assert!(t.last().unwrap().unterminated);
        assert!(tokenize("/* open").last().unwrap().unterminated);
    }

    #[test]
    fn words_and_numbers_and_accents() {
        assert_eq!(
            kinds("ação RDB$X 1.5e3 .5"),
            vec![(Word, "ação"), (Space, " "), (Word, "RDB$X"), (Space, " "), (Number, "1.5e3"), (Space, " "), (Number, ".5")]
        );
    }
}
