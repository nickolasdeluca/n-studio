//! A code formatter: SQL laid out again, by a style a team can share.
//!
//! It moves white space and changes the case of words, and nothing else: the
//! tokens that come out are the tokens that went in, which [`format`] checks
//! before it answers, handing the text back untouched when they are not. So
//! the worst it can do to a statement it does not follow is lay it out
//! poorly.
//!
//! It reads tokens, not a grammar: clauses start lines, lists and conditions
//! break as the style says, a subquery is indented inside its brackets, and
//! PSQL's blocks indent what they hold.

use serde::{Deserialize, Serialize};

use crate::lexer::{tokenize, TokenKind};

/// What is done to a word's letters
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Case {
    Upper,
    Lower,
    /// As written
    #[default]
    Keep,
}

impl Case {
    fn apply(self, word: &str) -> String {
        match self {
            Case::Upper => word.to_uppercase(),
            Case::Lower => word.to_lowercase(),
            Case::Keep => word.to_string(),
        }
    }
}

/// How the items of a list are laid out
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Lists {
    /// Each on its own line
    #[default]
    OnePerLine,
    /// One after the other, wrapped at the right margin
    Wrap,
}

/// A formatting style: what a team agrees on, kept in a file. A field a file
/// does not have takes its default, so a style written by an older version
/// still reads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Style {
    /// `select`, `from`, `begin`, and the built-in functions and types
    pub keywords: Case,
    /// Every other word. Quoted names are never touched.
    pub identifiers: Case,
    /// Spaces a level of indentation takes
    pub indent: usize,
    /// The column a line is wrapped before
    pub right_margin: usize,
    /// The columns of a SELECT and the assignments of an UPDATE
    pub select_list: Lists,
    /// The column and parameter definitions of CREATE TABLE and of a routine
    pub definitions: Lists,
    /// AND and OR each start a line in WHERE, HAVING and ON
    pub break_conditions: bool,
    /// A join's ON starts a line
    pub break_join_on: bool,
    pub space_after_comma: bool,
    /// `( a, b )` rather than `(a, b)`
    pub space_inside_brackets: bool,
    /// A subquery this short, or shorter, stays on its line
    pub inline_subquery: usize,
}

impl Default for Style {
    fn default() -> Self {
        Style {
            keywords: Case::Lower,
            identifiers: Case::Keep,
            indent: 2,
            right_margin: 100,
            select_list: Lists::OnePerLine,
            definitions: Lists::OnePerLine,
            break_conditions: true,
            break_join_on: false,
            space_after_comma: true,
            space_inside_brackets: false,
            inline_subquery: 50,
        }
    }
}

/// The words whose case is the keywords': the reserved ones, and those that
/// are not reserved and still read as the language's own.
#[rustfmt::skip]
const KEYWORDS: &[&str] = &[
    "ABS", "ACTIVE", "ADD", "ADMIN", "AFTER", "ALL", "ALTER", "AND", "ANY", "AS", "ASC", "ASCENDING", "AT", "AUTONOMOUS",
    "AVG",
    "BEFORE", "BEGIN", "BETWEEN", "BIGINT", "BINARY", "BLOB", "BLOCK", "BODY", "BOOLEAN", "BOTH", "BREAK", "BY",
    "CASCADE", "CASE", "CAST", "CHAR", "CHARACTER", "CHAR_LENGTH", "CHECK", "CLOSE", "COALESCE", "COLLATE",
    "COLUMN", "COMMENT", "COMMIT", "COMMITTED", "COMPUTED", "CONNECT", "CONSTRAINT", "CONTAINING", "CONTINUE",
    "COUNT", "CREATE", "CROSS", "CURRENT", "CURRENT_CONNECTION", "CURRENT_DATE", "CURRENT_ROLE", "CURRENT_TIME",
    "CURRENT_TIMESTAMP", "CURRENT_TRANSACTION", "CURRENT_USER", "CURSOR",
    "DATABASE", "DATE", "DATEADD", "DATEDIFF", "DAY", "DEC", "DECFLOAT", "DECIMAL", "DECLARE", "DECODE", "DEFAULT",
    "DELETE", "DELETING", "DESC", "DESCENDING", "DETERMINISTIC", "DISTINCT", "DO", "DOMAIN", "DOUBLE", "DROP",
    "ELSE", "END", "ENTRY_POINT", "ESCAPE", "EXCEPTION", "EXECUTE", "EXISTS", "EXIT", "EXTERNAL", "EXTRACT",
    "FALSE", "FETCH", "FILTER", "FIRST", "FLOAT", "FOR", "FOREIGN", "FROM", "FULL", "FUNCTION",
    "GDSCODE", "GENERATED", "GENERATOR", "GEN_ID", "GLOBAL", "GRANT", "GROUP",
    "HAVING", "HOUR",
    "IDENTITY", "IF", "IIF", "IN", "INACTIVE", "INDEX", "INNER", "INSERT", "INSERTING", "INT", "INT128", "INTEGER",
    "INTO", "IS", "ISOLATION",
    "JOIN",
    "KEY",
    "LAST", "LATERAL", "LEADING", "LEAVE", "LEFT", "LEVEL", "LIKE", "LIST", "LOCAL", "LOCALTIME", "LOCALTIMESTAMP",
    "LOCK", "LOWER",
    "MATCHED", "MAX", "MAXVALUE", "MERGE", "MIN", "MINUTE", "MINVALUE", "MODULE_NAME", "MONTH",
    "NATIONAL", "NATURAL", "NCHAR", "NEXT", "NO", "NOT", "NULL", "NULLIF", "NULLS", "NUMERIC",
    "OF", "OFFSET", "ON", "ONLY", "OPEN", "OPTION", "OR", "ORDER", "OUTER", "OVER",
    "PACKAGE", "PARAMETER", "PARTITION", "PLAN", "POSITION", "POST_EVENT", "PRECISION", "PRESERVE", "PRIMARY",
    "PRIVILEGES", "PROCEDURE",
    "READ", "REAL", "RECREATE", "RECURSIVE", "REFERENCES", "RELEASE", "RESTART", "RESTRICT", "RETAIN", "RETURN",
    "RETURNING", "RETURNING_VALUES", "RETURNS", "REVOKE", "RIGHT", "ROLE", "ROLLBACK", "ROW", "ROWS", "ROW_COUNT",
    "SAVEPOINT", "SCROLL", "SECOND", "SEGMENT", "SELECT", "SEQUENCE", "SET", "SIMILAR", "SIZE", "SKIP", "SMALLINT",
    "SNAPSHOT", "SOME", "SQLCODE", "SQLSTATE", "START", "STARTING", "STARTS", "STATEMENT", "SUBSTRING",
    "SUB_TYPE", "SUM", "SUSPEND",
    "TABLE", "TEMPORARY", "THEN", "TIME", "TIMESTAMP", "TO", "TRAILING", "TRANSACTION", "TRIGGER", "TRIM", "TRUE",
    "TYPE",
    "UNION", "UNIQUE", "UNKNOWN", "UPDATE", "UPDATING", "UPPER", "USER", "USING",
    "VALUE", "VALUES", "VARBINARY", "VARCHAR", "VARIABLE", "VARYING", "VIEW",
    "WAIT", "WHEN", "WHERE", "WHILE", "WINDOW", "WITH", "WITHOUT", "WORK", "WRITE",
    "YEAR", "ZONE",
];

/// Keywords a bracket is set apart from: `in (…)`, not `in(…)`. After any
/// other keyword a bracket opens its arguments: `count(…)`, `varchar(…)`.
#[rustfmt::skip]
const BEFORE_BRACKET: &[&str] = &[
    "ALL", "AND", "ANY", "AS", "BY", "CHECK", "DEFAULT", "DO", "ELSE", "EXISTS", "FOR", "FROM", "IF", "IN", "INSERT", "INTO",
    "IS",
    "JOIN", "KEY", "LIKE", "NOT", "ON", "OR", "OVER", "REFERENCES", "RETURNS", "SELECT", "SET", "SOME", "THEN",
    "UNION", "UNIQUE", "USING", "VALUES", "WHEN", "WHERE", "WHILE", "WITH",
];

fn is_keyword(upper: &str) -> bool {
    KEYWORDS.binary_search(&upper).is_ok()
}

/// A token of code or a comment, and the space that came before it
struct Tok<'a> {
    kind: TokenKind,
    text: &'a str,
    /// A word in capitals, to tell keywords by; empty for anything else
    upper: String,
    /// Space, of any kind, before it
    gap: bool,
    /// Line ends in that space
    lines: usize,
}

impl Tok<'_> {
    fn is(&self, punct: &str) -> bool {
        self.kind == TokenKind::Punct && self.text == punct
    }

    fn comment(&self) -> bool {
        matches!(self.kind, TokenKind::LineComment | TokenKind::BlockComment)
    }
}

fn tokens(text: &str) -> Option<Vec<Tok<'_>>> {
    let mut list = Vec::new();
    let (mut gap, mut lines) = (false, 0);
    for token in tokenize(text) {
        // What ends inside a string or a comment is being typed: leave it
        if token.unterminated {
            return None;
        }
        let slice = &text[token.range.clone()];
        if token.kind == TokenKind::Space {
            gap = true;
            lines = slice.matches('\n').count();
            continue;
        }
        let upper = if token.kind == TokenKind::Word { slice.to_uppercase() } else { String::new() };
        list.push(Tok { kind: token.kind, text: slice, upper, gap, lines });
        (gap, lines) = (false, 0);
    }
    Some(list)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Clause {
    None,
    Select,
    From,
    /// WHERE and HAVING: conditions
    Where,
    Set,
    Insert,
    Other,
}

/// A query: the statement itself, or a subquery in its brackets
struct Level {
    /// Where its clauses start
    indent: usize,
    clause: Clause,
    /// Brackets open inside it that are not queries of their own
    brackets: usize,
    /// CASE … END open inside it
    cases: usize,
    /// Past BETWEEN, before its AND
    between: bool,
    /// Where AND and OR go, in a condition
    condition: Option<usize>,
    /// The next token starts a list item on its own line
    item: bool,
    /// The token after FIRST or SKIP: their argument
    argument: bool,
    /// The list item waits for this many brackets to close (FIRST (…))
    held: Option<usize>,
    /// A short subquery, kept on its line: nothing in it breaks
    inline: bool,
}

impl Level {
    fn new(indent: usize) -> Self {
        Level {
            indent,
            clause: Clause::None,
            brackets: 0,
            cases: 0,
            between: false,
            condition: None,
            item: false,
            argument: false,
            held: None,
            inline: false,
        }
    }

    /// Not inside brackets or a CASE: where clauses are
    fn top(&self) -> bool {
        self.brackets == 0 && self.cases == 0 && !self.inline
    }
}

enum Bracket {
    Plain,
    /// A subquery: a level of its own, closed on the line `back` indents
    Query { back: usize, inline: bool },
    /// Definitions, one per line at `inner`, closed at `back`
    Definitions { inner: usize, back: usize },
}

/// What PSQL indents
enum Frame {
    /// BEGIN … END: `at` is BEGIN's own indentation
    Block { at: usize },
    /// The one statement after THEN, ELSE or DO, up to its `;`
    Single,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Start {
    No,
    /// The next token starts a statement, on a line of its own
    Line,
    /// It starts one where it is: `else if`
    Inline,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Header {
    None,
    /// CREATE PROCEDURE and the like, up to the AS of its body
    Routine,
    /// CREATE TABLE, up to its bracket
    Table,
}

struct Out {
    text: String,
    width: usize,
    /// Characters on the line so far
    column: usize,
    /// Nothing but indentation on the line
    fresh: bool,
    /// The indentation of the line, in levels
    indent: usize,
    /// Where a line too long goes on
    continuation: usize,
}

impl Out {
    fn line(&mut self, indent: usize, blank: bool) {
        if !self.text.is_empty() {
            let kept = self.text.trim_end_matches(' ').len();
            self.text.truncate(kept);
            self.text.push('\n');
            if blank {
                self.text.push('\n');
            }
        }
        self.text.push_str(&" ".repeat(indent * self.width));
        self.column = indent * self.width;
        self.fresh = true;
        self.indent = indent;
        self.continuation = indent + 1;
    }

    fn put(&mut self, text: &str) {
        self.text.push_str(text);
        self.column = match text.rfind('\n') {
            Some(at) => text[at + 1..].chars().count(),
            None => self.column + text.chars().count(),
        };
        self.fresh = false;
    }
}

struct Formatter<'a> {
    style: &'a Style,
    toks: Vec<Tok<'a>>,
    out: Out,
    levels: Vec<Level>,
    brackets: Vec<Bracket>,
    frames: Vec<Frame>,
    start: Start,
    /// The first word of the statement, in capitals
    first: String,
    header: Header,
    /// The statement terminator: `;` until a SET TERM
    terminator: String,
    /// A line comment was written: whatever comes next starts a line
    after_comment: bool,
    /// The next token starts a line at this indentation: a definition
    definition: Option<usize>,
    /// A statement whose words are not clauses: GRANT SELECT … ON … FROM
    flat: bool,
}

impl<'a> Formatter<'a> {
    /// Where a statement starts, inside the blocks open around it
    fn base(&self) -> usize {
        self.frames.len()
    }

    fn level(&mut self) -> &mut Level {
        self.levels.last_mut().expect("the statement's own level is always there")
    }

    /// The word, in capitals, of the next token of code after `i`
    fn next_word(&self, i: usize) -> &str {
        self.toks[i + 1..].iter().find(|t| !t.comment()).map_or("", |t| t.upper.as_str())
    }

    /// The word before `i`, comments stepped over
    fn previous_word(&self, i: usize) -> &str {
        self.toks[..i].iter().rev().find(|t| !t.comment()).map_or("", |t| t.upper.as_str())
    }

    /// `[natural] [inner | left | right | full | cross] [outer] join`
    /// starting at `i`, told from the functions LEFT and RIGHT
    fn join_starts(&self, i: usize) -> bool {
        let mut words = self.toks[i..].iter().filter(|t| !t.comment()).map(|t| t.upper.as_str());
        let first = words.next().unwrap_or("");
        if !matches!(first, "JOIN" | "INNER" | "LEFT" | "RIGHT" | "FULL" | "CROSS" | "NATURAL") {
            return false;
        }
        let mut word = first;
        for _ in 0..3 {
            if word == "JOIN" {
                return true;
            }
            if !matches!(word, "INNER" | "LEFT" | "RIGHT" | "FULL" | "CROSS" | "NATURAL" | "OUTER") {
                return false;
            }
            word = words.next().unwrap_or("");
        }
        word == "JOIN"
    }

    /// Past the join's own words, so that they are one line's start
    fn in_join_words(&self, i: usize) -> bool {
        matches!(self.previous_word(i), "INNER" | "LEFT" | "RIGHT" | "FULL" | "CROSS" | "NATURAL" | "OUTER")
            && matches!(self.toks[i].upper.as_str(), "JOIN" | "OUTER" | "INNER" | "LEFT" | "RIGHT" | "FULL" | "CROSS")
    }

    /// Whether the bracket at `i` holds a query, and how long it is on one
    /// line (`None` when it holds a comment or does not close)
    fn query_in_bracket(&self, i: usize) -> Option<Option<usize>> {
        if !matches!(self.next_word(i), "SELECT" | "WITH") {
            return None;
        }
        let (mut depth, mut length) = (0usize, 0usize);
        for t in &self.toks[i..] {
            if t.comment() {
                return Some(None);
            }
            length += t.text.chars().count() + 1;
            if t.is("(") {
                depth += 1;
            } else if t.is(")") {
                depth -= 1;
                if depth == 0 {
                    return Some(Some(length));
                }
            }
        }
        Some(None)
    }

    /// What the statement that starts at `i` is, for its brackets
    fn header_at(&self, i: usize) -> Header {
        let mut words = self.toks[i..].iter().filter(|t| !t.comment()).map(|t| t.upper.as_str());
        match words.next().unwrap_or("") {
            "EXECUTE" if words.next() == Some("BLOCK") => Header::Routine,
            "CREATE" | "ALTER" | "RECREATE" | "DECLARE" => {
                for word in words.take(4) {
                    match word {
                        "PROCEDURE" | "FUNCTION" => return Header::Routine,
                        "TABLE" => return Header::Table,
                        "OR" | "ALTER" | "GLOBAL" | "TEMPORARY" => {}
                        _ => break,
                    }
                }
                Header::None
            }
            _ => Header::None,
        }
    }

    fn statement_ends(&mut self) {
        while matches!(self.frames.last(), Some(Frame::Single)) {
            self.frames.pop();
        }
        self.start = Start::Line;
        self.brackets.clear();
        self.levels = vec![Level::new(self.base())];
    }

    /// The body after THEN, ELSE or DO: a block, or one statement indented
    fn body(&mut self, i: usize) {
        match self.next_word(i) {
            "BEGIN" => {}
            "IF" if self.toks[i].upper == "ELSE" => {
                self.start = Start::Inline;
                self.levels = vec![Level::new(self.base())];
                return;
            }
            _ => self.frames.push(Frame::Single),
        }
        self.start = Start::Line;
        self.levels = vec![Level::new(self.base())];
    }

    /// Whether there is a space between two tokens on a line
    fn space(&self, previous: &Tok, current: &Tok) -> bool {
        use TokenKind::*;
        if previous.is(",") {
            return self.style.space_after_comma;
        }
        if current.is(",") || current.is(";") || current.is(".") || previous.is(".") {
            return false;
        }
        if previous.is("(") {
            return self.style.space_inside_brackets && !current.is(")");
        }
        if current.is(")") {
            return self.style.space_inside_brackets;
        }
        if current.is("[") || current.is("]") || previous.is("[") || previous.is("]") {
            return current.gap;
        }
        if current.is("(") {
            return match previous.kind {
                Word if is_keyword(&previous.upper) => BEFORE_BRACKET.binary_search(&previous.upper.as_str()).is_ok(),
                // A name: a call, or a table and its column list, as written
                Word | Quoted => current.gap,
                _ => !previous.is("(") || self.style.space_inside_brackets,
            };
        }
        let comparison = |t: &Tok| t.kind == Punct && matches!(t.text, "=" | "<" | ">" | "|" | "!");
        if current.kind == Punct || previous.kind == Punct {
            // What is written together stays together: <=, ||, -1, t.*
            if current.kind == Punct && previous.kind == Punct {
                return current.gap;
            }
            if comparison(current) || comparison(previous) {
                return true;
            }
            return current.gap;
        }
        // A word and a string written as one are one: x'0A', _utf8'…'
        if matches!(current.kind, String | Quoted) && previous.kind == Word {
            return current.gap;
        }
        true
    }

    fn written(&self, i: usize) -> String {
        let t = &self.toks[i];
        if t.kind != TokenKind::Word {
            return t.text.to_string();
        }
        let qualified = (i > 0 && self.toks[i - 1].is(".")) || self.toks.get(i + 1).is_some_and(|n| n.is("."));
        if !qualified && is_keyword(&t.upper) {
            self.style.keywords.apply(t.text)
        } else {
            self.style.identifiers.apply(t.text)
        }
    }

    fn run(mut self) -> String {
        let one_per_line = self.style.select_list == Lists::OnePerLine;
        let mut i = 0;
        while i < self.toks.len() {
            let blank = self.toks[i].lines > 1;
            let lines = self.toks[i].lines;

            // Comments keep the line they had: their own, or the end of one
            if self.toks[i].comment() {
                let own_line = lines > 0 || self.out.text.is_empty() || self.after_comment;
                if own_line {
                    let indent = if self.start != Start::No { self.base() } else { self.out.continuation };
                    let indent = self.definition.unwrap_or(indent);
                    if !self.out.text.is_empty() {
                        self.out.line(indent, blank);
                    }
                } else {
                    self.out.put(" ");
                }
                self.out.put(self.toks[i].text);
                self.after_comment = self.toks[i].kind == TokenKind::LineComment
                    || self.toks.get(i + 1).is_some_and(|next| next.lines > 0);
                i += 1;
                continue;
            }

            // SET TERM is not SQL: it stays as it is, and what ends a
            // statement changes
            if self.start != Start::No && self.toks[i].upper == "SET" && self.next_word(i) == "TERM" {
                let end = self.toks[i + 1..].iter().position(|t| t.lines > 0).map_or(self.toks.len(), |n| i + 1 + n);
                if !self.out.text.is_empty() {
                    self.out.line(self.base(), blank);
                }
                let mut line = String::new();
                for (n, t) in self.toks[i..end].iter().enumerate() {
                    if n > 0 && t.gap {
                        line.push(' ');
                    }
                    line.push_str(t.text);
                }
                let rest = line[line.to_uppercase().find("TERM").map_or(0, |at| at + 4)..].trim();
                let next = rest.strip_suffix(self.terminator.as_str()).unwrap_or(rest).trim();
                if !next.is_empty() && !next.contains(' ') {
                    self.terminator = next.to_string();
                }
                self.out.put(&line);
                self.statement_ends();
                i = end;
                continue;
            }

            let upper = self.toks[i].upper.clone();
            let word = upper.as_str();
            // A line break before the token, and where it indents to
            let mut line: Option<usize> = None;
            let mut block_ended = false;

            if self.start != Start::No {
                if self.start == Start::Line {
                    line = Some(self.base());
                }
                self.start = Start::No;
                self.first = upper.clone();
                self.header = self.header_at(i);
                self.flat = matches!(word, "GRANT" | "REVOKE" | "COMMENT" | "DROP" | "SET" | "COMMIT" | "ROLLBACK");
                if word == "FOR" && matches!(self.next_word(i), "SELECT" | "WITH" | "EXECUTE") {
                    // FOR SELECT … DO: the query hangs under FOR
                    self.level().indent = self.base() + 1;
                }
            }

            let top = self.levels.last().is_some_and(Level::top) && !self.flat;
            let outermost = self.levels.len() == 1 && self.brackets.is_empty();
            let indent = self.levels.last().map_or(0, |l| l.indent);
            let clause = self.levels.last().map_or(Clause::None, |l| l.clause);
            let merge = self.first == "MERGE";

            // The item of a list that goes one per line
            if self.level().item && !self.level().inline {
                if self.level().argument {
                    self.level().argument = false;
                    if self.toks[i].is("(") {
                        self.level().held = Some(self.levels.last().map_or(0, |l| l.brackets));
                        self.level().item = false;
                    }
                } else if matches!(word, "DISTINCT" | "ALL") && self.previous_word(i) == "SELECT" {
                } else if matches!(word, "FIRST" | "SKIP") && clause == Clause::Select {
                    self.level().argument = true;
                } else {
                    self.level().item = false;
                    line = Some(indent + 1);
                }
            }
            if let Some(at) = self.definition.take() {
                if !self.toks[i].is(")") {
                    line = Some(at);
                }
            }

            match word {
                "BEGIN" => {
                    let at = self.base();
                    line = Some(at);
                    self.frames.push(Frame::Block { at });
                    self.statement_ends();
                }
                "END" if self.levels.last().is_some_and(|l| l.cases > 0) => self.level().cases -= 1,
                "END" => {
                    let mut at = 0;
                    while let Some(frame) = self.frames.pop() {
                        if let Frame::Block { at: begin } = frame {
                            at = begin;
                            break;
                        }
                    }
                    // The statement a THEN or a DO held ends with its block
                    while matches!(self.frames.last(), Some(Frame::Single)) {
                        self.frames.pop();
                    }
                    line = Some(at);
                    self.levels = vec![Level::new(self.base())];
                    self.brackets.clear();
                    block_ended = true;
                }
                // FOR SELECT's DO closes the query that hangs under FOR
                "DO" if top && outermost && self.first == "FOR" => line = Some(self.base()),
                "CASE" => self.level().cases += 1,
                "BETWEEN" => self.level().between = true,
                "AS" if outermost && top && matches!(self.next_word(i), "BEGIN" | "DECLARE") => {
                    line = Some(self.base());
                    self.header = Header::None;
                    self.start = Start::Line;
                }
                "RETURNS" if outermost && self.header == Header::Routine => line = Some(self.base()),
                "ELSE" if top && outermost && self.frames_hold_if() => {
                    line = Some(self.base());
                }
                "SELECT" if top => {
                    line = Some(indent);
                    self.level().clause = Clause::Select;
                    self.level().condition = None;
                    self.level().item = one_per_line;
                }
                "FROM" if top && self.previous_word(i) != "DELETE" && clause != Clause::None => {
                    line = Some(indent);
                    self.level().clause = Clause::From;
                    self.level().condition = None;
                }
                "WHERE" | "HAVING" if top => {
                    line = Some(indent);
                    self.level().clause = Clause::Where;
                    self.level().condition = Some(indent + 1);
                }
                "GROUP" | "ORDER" if top && self.next_word(i) == "BY" => {
                    line = Some(indent);
                    self.level().clause = Clause::Other;
                    self.level().condition = None;
                }
                "UNION" if top => {
                    line = Some(indent);
                    self.level().clause = Clause::None;
                    self.level().condition = None;
                }
                "INTO" if top => {
                    if matches!(self.previous_word(i), "INSERT" | "MERGE") {
                        self.level().clause = Clause::Insert;
                    } else if clause != Clause::None {
                        line = Some(indent);
                        self.level().clause = Clause::Other;
                        self.level().condition = None;
                    }
                }
                "VALUES" | "RETURNING" if top && !merge => {
                    line = Some(indent);
                    self.level().clause = Clause::Other;
                    self.level().condition = None;
                }
                "USING" if top && merge => {
                    line = Some(indent);
                    self.level().clause = Clause::From;
                }
                "WHEN" if top && merge => {
                    line = Some(indent);
                    self.level().condition = None;
                }
                "UPDATE" | "DELETE" | "INSERT" if top && clause == Clause::None => self.level().clause = Clause::Other,
                "SET" if top && self.first == "UPDATE" && clause == Clause::Other && self.previous_word(i) != "UPDATE" => {
                    line = Some(indent);
                    self.level().clause = Clause::Set;
                    self.level().item = one_per_line;
                }
                "ROWS" | "PLAN" | "OFFSET" | "FETCH" | "WINDOW"
                    if top && !matches!(clause, Clause::None | Clause::Insert | Clause::Set) =>
                {
                    line = Some(indent);
                    self.level().clause = Clause::Other;
                    self.level().condition = None;
                }
                "ON" if top && clause == Clause::From => {
                    if self.style.break_join_on {
                        line = Some(indent + 2);
                    }
                    self.level().condition = Some(indent + 2);
                }
                "AND" | "OR" if top => {
                    let between = word == "AND" && std::mem::take(&mut self.level().between);
                    if let (Some(at), true, false) = (self.level().condition, self.style.break_conditions, between) {
                        line = Some(at);
                    }
                }
                _ if top && clause == Clause::From && self.join_starts(i) && !self.in_join_words(i) => {
                    line = Some(indent + 1);
                    self.level().condition = None;
                }
                _ => {}
            }
            if word == "AND" && !top {
                self.level().between = false;
            }

            // Brackets: a subquery is a level, definitions go one per line
            let mut after: Option<usize> = None;
            if self.toks[i].is("(") {
                let definitions = outermost && top && self.header != Header::None;
                match self.query_in_bracket(i) {
                    Some(length) => {
                        let inline = self.levels.last().is_some_and(|l| l.inline)
                            || length.is_some_and(|n| n <= self.style.inline_subquery);
                        self.brackets.push(Bracket::Query { back: 0, inline });
                        let mut level = Level::new(0);
                        level.inline = inline;
                        self.levels.push(level);
                    }
                    None if definitions && self.style.definitions == Lists::OnePerLine => {
                        let back = self.base();
                        self.brackets.push(Bracket::Definitions { inner: back + 1, back });
                        after = Some(back + 1);
                        if self.header == Header::Table {
                            self.header = Header::None;
                        }
                    }
                    None => {
                        self.brackets.push(Bracket::Plain);
                        self.level().brackets += 1;
                    }
                }
            } else if self.toks[i].is(")") {
                match self.brackets.pop() {
                    Some(Bracket::Query { back, inline }) => {
                        if self.levels.len() > 1 {
                            self.levels.pop();
                        }
                        if !inline {
                            line = Some(back);
                        }
                    }
                    Some(Bracket::Definitions { back, .. }) => line = Some(back),
                    Some(Bracket::Plain) => {
                        let level = self.level();
                        level.brackets = level.brackets.saturating_sub(1);
                        if level.held == Some(level.brackets) {
                            level.held = None;
                            level.item = true;
                        }
                    }
                    None => {}
                }
            } else if self.toks[i].is(",") {
                let level = self.levels.last().expect("always one");
                if matches!(self.brackets.last(), Some(Bracket::Definitions { .. })) && level.brackets == 0 {
                    if let Some(Bracket::Definitions { inner, .. }) = self.brackets.last() {
                        after = Some(*inner);
                    }
                } else if level.top() && one_per_line && matches!(level.clause, Clause::Select | Clause::Set) {
                    self.level().item = true;
                }
            }

            // The token itself: on a new line, or after what the line has
            let text = self.written(i);
            if self.after_comment && line.is_none() {
                line = Some(self.out.continuation);
            }
            self.after_comment = false;
            match line {
                Some(at) if !self.out.text.is_empty() => {
                    if !self.out.fresh || self.out.indent != at || blank {
                        let keep = self.out.fresh && !blank;
                        if keep {
                            // An empty line was started: move it, not add one
                            let kept = self.out.text.trim_end_matches(' ').len();
                            self.out.text.truncate(kept);
                            self.out.text.push_str(&" ".repeat(at * self.out.width));
                            self.out.column = at * self.out.width;
                            self.out.indent = at;
                            self.out.continuation = at + 1;
                        } else {
                            self.out.line(at, blank && !self.out.fresh);
                        }
                    }
                }
                Some(at) => {
                    self.out.indent = at;
                    self.out.continuation = at + 1;
                }
                None => {
                    let spaced = i > 0 && !self.out.fresh && self.space(&self.toks[i - 1], &self.toks[i]);
                    let length = text.chars().count() + usize::from(spaced);
                    let closes = self.toks[i].kind == TokenKind::Punct && matches!(self.toks[i].text, ")" | "," | ";" | ".");
                    if spaced && !closes && self.out.column + length > self.style.right_margin {
                        let at = self.out.continuation;
                        self.out.line(at, false);
                        self.out.continuation = at;
                    } else if spaced {
                        self.out.put(" ");
                    }
                }
            }
            self.out.put(&text);

            // What the token leaves for the next one
            if let Some(Bracket::Query { back, inline }) = self.brackets.last_mut() {
                if self.toks[i].is("(") && *back == 0 && self.levels.last().is_some_and(|l| l.clause == Clause::None) {
                    *back = self.out.indent;
                    let inner = self.out.indent + 1;
                    let inline = *inline;
                    let level = self.level();
                    if level.indent == 0 && !inline {
                        level.indent = inner;
                    }
                }
            }
            if let Some(at) = after {
                self.definition = Some(at);
            }
            let ends = self.levels.len() == 1
                && self.brackets.is_empty()
                && self.toks[i].kind == TokenKind::Punct
                && (self.toks[i].is(";") || self.terminator.starts_with(self.toks[i].text) || self.terminator.ends_with(self.toks[i].text));
            if ends {
                self.statement_ends();
            } else if top && outermost {
                match word {
                    "THEN" if self.first == "IF" => self.body(i),
                    "ELSE" if self.frames_hold_if() => self.body(i),
                    "DO" if matches!(self.first.as_str(), "FOR" | "WHILE" | "WHEN" | "IN") => self.body(i),
                    _ => {}
                }
            }
            // A block's END ends a statement too, with or without a `;`
            if block_ended && self.toks.get(i + 1).is_some_and(|next| next.kind != TokenKind::Punct) {
                self.start = Start::Line;
            }
            i += 1;
        }
        let kept = self.out.text.trim_end_matches(' ').len();
        self.out.text.truncate(kept);
        self.out.text
    }

    /// ELSE is IF's: any ELSE outside a CASE, in a statement that is not a
    /// MERGE's or a query's own
    fn frames_hold_if(&self) -> bool {
        self.levels.len() == 1 && self.levels[0].cases == 0 && self.levels[0].clause == Clause::None
    }
}

/// The tokens that carry meaning, words in capitals: what formatting must
/// leave as it found
fn meaning(text: &str) -> Vec<String> {
    tokenize(text)
        .into_iter()
        .filter(|t| t.kind != TokenKind::Space)
        .map(|t| {
            let slice = &text[t.range];
            match t.kind {
                TokenKind::Word => slice.to_uppercase(),
                // A comment ends where its line does, whatever follows
                TokenKind::LineComment => slice.trim_end().to_string(),
                _ => slice.to_string(),
            }
        })
        .collect()
}

/// `text` laid out by `style`: a statement, or a script of them. The text
/// itself comes back when it ends inside a string or a comment, or when the
/// result would not hold the same tokens.
pub fn format(text: &str, style: &Style) -> String {
    let Some(toks) = tokens(text) else { return text.to_string() };
    if toks.is_empty() {
        return text.to_string();
    }
    let style = Style {
        indent: style.indent.clamp(1, 8),
        right_margin: style.right_margin.clamp(40, 400),
        ..style.clone()
    };
    let formatter = Formatter {
        style: &style,
        toks,
        out: Out { text: String::new(), width: style.indent, column: 0, fresh: true, indent: 0, continuation: 1 },
        levels: vec![Level::new(0)],
        brackets: Vec::new(),
        frames: Vec::new(),
        start: Start::Inline,
        first: String::new(),
        header: Header::None,
        terminator: ";".into(),
        after_comment: false,
        definition: None,
        flat: false,
    };
    let mut formatted = formatter.run();
    // What surrounded the text stays: its indentation is the caller's
    let lead = &text[..text.len() - text.trim_start().len()];
    let trail = &text[text.trim_end().len()..];
    formatted = format!("{lead}{formatted}{trail}");
    if meaning(&formatted) == meaning(text) {
        formatted
    } else {
        text.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default(text: &str) -> String {
        format(text, &Style::default())
    }

    #[test]
    fn keywords_are_sorted() {
        assert!(KEYWORDS.windows(2).all(|w| w[0] < w[1]), "KEYWORDS: not sorted, or a word twice");
        assert!(BEFORE_BRACKET.windows(2).all(|w| w[0] < w[1]), "BEFORE_BRACKET: not sorted, or a word twice");
        assert!(BEFORE_BRACKET.iter().all(|w| is_keyword(w)));
    }

    #[test]
    fn a_select_is_laid_out_by_its_clauses() {
        let sql = "SELECT p.id, p.total , c.nome FROM pedido p inner join cliente c on c.id=p.cliente \
                   left outer join cidade ci on ci.id = c.cidade and ci.uf = 'SC' \
                   WHERE p.total>100 and (p.emissao between :de and :ate or p.pago = 1) and c.nome like 'A%' \
                   group by p.id, p.total, c.nome having count(*) > 1 ORDER BY 2 desc, 1 rows 10";
        assert_eq!(
            default(sql),
            "select
  p.id,
  p.total,
  c.nome
from pedido p
  inner join cliente c on c.id = p.cliente
  left outer join cidade ci on ci.id = c.cidade
    and ci.uf = 'SC'
where p.total > 100
  and (p.emissao between :de and :ate or p.pago = 1)
  and c.nome like 'A%'
group by p.id, p.total, c.nome
having count(*) > 1
order by 2 desc, 1
rows 10"
        );
    }

    #[test]
    fn the_style_decides_case_lists_and_breaks() {
        let style = Style {
            keywords: Case::Upper,
            identifiers: Case::Lower,
            indent: 4,
            select_list: Lists::Wrap,
            break_conditions: false,
            break_join_on: true,
            space_after_comma: false,
            space_inside_brackets: true,
            ..Style::default()
        };
        assert_eq!(
            format("select A, \"Misto\", coalesce(B,0) from T join X on X.ID = T.ID where A = 1 and B in (1,2)", &style),
            "SELECT a,\"Misto\",COALESCE( b,0 )
FROM t
    JOIN x
        ON x.id = t.id
WHERE a = 1 AND b IN ( 1,2 )"
        );
    }

    #[test]
    fn a_long_line_wraps_at_the_margin() {
        let style = Style { right_margin: 40, select_list: Lists::Wrap, ..Style::default() };
        let formatted = format("select alfa, beta, gama, delta, epsilon, zeta, eta, teta, iota from t", &style);
        assert_eq!(formatted, "select alfa, beta, gama, delta, epsilon,\n  zeta, eta, teta, iota\nfrom t");
        assert!(formatted.lines().all(|l| l.len() <= 40));
    }

    #[test]
    fn a_subquery_is_indented_in_its_brackets_unless_it_is_short() {
        assert_eq!(
            default(
                "select * from t where exists (select 1 from x where x.t = t.id) and t.id in \
                 (select y.t from y join z on z.id = y.z where z.ativo = 1 and y.valor > 1000)"
            ),
            "select
  *
from t
where exists (select 1 from x where x.t = t.id)
  and t.id in (
    select
      y.t
    from y
      join z on z.id = y.z
    where z.ativo = 1
      and y.valor > 1000
  )"
        );
    }

    #[test]
    fn insert_update_delete_and_merge() {
        assert_eq!(
            default("insert into t (a, b) values (1, 'x') returning id"),
            "insert into t (a, b)\nvalues (1, 'x')\nreturning id"
        );
        assert_eq!(
            default("insert into t(a) select a from u where a>1"),
            "insert into t(a)\nselect\n  a\nfrom u\nwhere a > 1"
        );
        assert_eq!(
            default("update t set a = 1, b = b+1 where id = :id"),
            "update t\nset\n  a = 1,\n  b = b+1\nwhere id = :id"
        );
        assert_eq!(default("delete from t where a is null"), "delete from t\nwhere a is null");
        assert_eq!(
            default(
                "merge into t using s on s.id = t.id when matched then update set t.a = s.a \
                 when not matched then insert (id, a) values (s.id, s.a)"
            ),
            "merge into t
using s on s.id = t.id
when matched then update set t.a = s.a
when not matched then insert (id, a) values (s.id, s.a)"
        );
    }

    #[test]
    fn union_and_a_cte() {
        assert_eq!(
            default("with a as (select id from t where x = 1 and y = 2 and z = 3 and w = 4) select id from a union all select 0 from rdb$database"),
            "with a as (
  select
    id
  from t
  where x = 1
    and y = 2
    and z = 3
    and w = 4
)
select
  id
from a
union all
select
  0
from rdb$database"
        );
    }

    #[test]
    fn psql_blocks_indent_what_they_hold() {
        let sql = "create or alter procedure P_TOTAL (pedido integer, taxa numeric(9,2) = 0) returns (total numeric(18,2), itens integer) as \
                   declare variable v integer; declare x varchar(10); begin total = 0; \
                   for select i.qtd * i.preco, i.item from pedido_item i where i.pedido = :pedido into :v, :itens do begin \
                   if (v > 0 and itens is not null) then total = total + v; else begin total = total - 1; itens = 0; end \
                   suspend; end \
                   while (total < 10) do total = total + case when taxa > 0 then taxa else 1 end; \
                   if (total = 0) then exception e_vazio; else if (total < 0) then exit; \
                   when any do begin total = null; end end";
        assert_eq!(
            default(sql),
            "create or alter procedure P_TOTAL (
  pedido integer,
  taxa numeric(9, 2) = 0
)
returns (
  total numeric(18, 2),
  itens integer
)
as
declare variable v integer;
declare x varchar(10);
begin
  total = 0;
  for
    select
      i.qtd * i.preco,
      i.item
    from pedido_item i
    where i.pedido = :pedido
    into :v, :itens
  do
  begin
    if (v > 0 and itens is not null) then
      total = total + v;
    else
    begin
      total = total - 1;
      itens = 0;
    end
    suspend;
  end
  while (total < 10) do
    total = total + case when taxa > 0 then taxa else 1 end;
  if (total = 0) then
    exception e_vazio;
  else if (total < 0) then
    exit;
  when any do
  begin
    total = null;
  end
end"
        );
    }

    #[test]
    fn a_script_keeps_its_terminators_and_its_blank_lines() {
        let sql = "set term ^ ;\n\ncreate trigger t_bi for t active before insert position 0 as begin if (new.id is null) then new.id = gen_id(g, 1); end^\n\n\nset term ; ^\ncommit;\nselect 1 from rdb$database;";
        assert_eq!(
            default(sql),
            "set term ^ ;

create trigger t_bi for t active before insert position 0
as
begin
  if (new.id is null) then
    new.id = gen_id(g, 1);
end^

set term ; ^
commit;
select
  1
from rdb$database;"
        );
    }

    #[test]
    fn create_table_lists_its_columns() {
        assert_eq!(
            default("create table t (id integer not null, nome varchar(60) default 'x', constraint pk_t primary key (id), constraint fk foreign key (c) references u (id) on delete cascade)"),
            "create table t (
  id integer not null,
  nome varchar(60) default 'x',
  constraint pk_t primary key (id),
  constraint fk foreign key (c) references u (id) on delete cascade
)"
        );
        assert_eq!(default("create index i on t (a, b)"), "create index i on t (a, b)");
        // Words that are clauses elsewhere
        assert_eq!(
            default("GRANT SELECT, UPDATE (a, b) ON t TO ROLE r WITH GRANT OPTION; revoke all on t from u;"),
            "grant select, update(a, b) on t to role r with grant option;\nrevoke all on t from u;"
        );
    }

    #[test]
    fn comments_stay_where_they_were() {
        assert_eq!(
            default("select a, -- the first\n b /* inline */ from t -- trailing\nwhere a = 1\n-- alone\nand b = 2"),
            "select
  a, -- the first
  b /* inline */
from t -- trailing
where a = 1
  -- alone
  and b = 2"
        );
    }

    #[test]
    fn what_is_written_together_stays_together() {
        let sql = "select x'0A', _utf8'é', -1, a||b, t.*, a<=b, a<>b, a [1], n'x', q'{it's}' from t";
        let formatted = format(sql, &Style { select_list: Lists::Wrap, ..Style::default() });
        assert_eq!(formatted, "select x'0A', _utf8'é', -1, a || b, t.*, a <= b, a <> b, a [1], n'x', q'{it's}'\nfrom t");
    }

    #[test]
    fn text_that_cannot_be_followed_comes_back_as_it_was() {
        for sql in ["select 'open", "select /* open", "", "   \n", "select \"open"] {
            assert_eq!(default(sql), sql);
        }
    }

    #[test]
    fn formatting_twice_changes_nothing_more_and_no_token() {
        let corpus = [
            include_str!("../../../docker/seed/00-fixture.sql"),
            "select first 10 skip (5) distinct a, b from t where a in (select max(x) from y group by z) plan (t natural)",
            "execute block (x integer = ?) returns (y integer) as begin y = x; suspend; end",
            "select case a when 1 then 'x' else 'y' end, extract(year from d), trim(both ' ' from s), substring(s from 1 for 2) from t for update with lock",
            "grant select, insert on t to role r; revoke all on t from public;",
            "alter table t add c integer default 0 not null, alter d type varchar(20), drop e;",
            "select a from t where a = any (select 1 from u) and not (b is null) order by a nulls last offset 5 rows fetch first 3 rows only",
            "recreate view v (a, b) as select a, b from t where exists (select * from u where u.a = t.a) with check option",
            "if (a = 1) then if (b = 2) then c = 3; else c = 4;",
            "select count(*) over (partition by a order by b rows between 1 preceding and current row) from t",
        ];
        let styles = [
            Style::default(),
            Style { keywords: Case::Upper, identifiers: Case::Lower, select_list: Lists::Wrap, right_margin: 60, ..Style::default() },
            Style { definitions: Lists::Wrap, break_conditions: false, break_join_on: true, space_inside_brackets: true, indent: 4, inline_subquery: 0, ..Style::default() },
        ];
        for style in &styles {
            for sql in corpus {
                let once = format(sql, style);
                assert_eq!(meaning(&once), meaning(sql), "tokens changed");
                assert_ne!(once, sql, "handed back untouched: {sql}");
                assert_eq!(format(&once, style), once, "not stable:\n{once}");
            }
        }
    }

    #[test]
    fn a_style_reads_from_a_file_that_has_only_some_of_it() {
        let style: Style = serde_json::from_str(r#"{"keywords":"upper","rightMargin":80,"somethingNewer":true}"#).unwrap();
        assert_eq!(style, Style { keywords: Case::Upper, right_margin: 80, ..Style::default() });
        let text = serde_json::to_string(&Style::default()).unwrap();
        assert_eq!(serde_json::from_str::<Style>(&text).unwrap(), Style::default());
    }
}
