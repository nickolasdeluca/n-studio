//! A PSQL routine's body taken apart into its statements, for the debugger
//! to run one at a time. Only the flow is understood here: what to do next.
//! Expressions and SQL stay as text, since the server is what evaluates
//! them; [`bind`] turns the variables they name into parameters.
//!
//! What the debugger cannot run is still parsed, into [`Stmt::Unsupported`],
//! so that it can say so at the statement and run everything before it.

use std::ops::Range;

use crate::lexer::{tokenize, TokenKind};

/// Byte offsets into the source
pub type Span = Range<usize>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    pub stmt: Stmt,
    /// What to show as running: all of a simple statement, the head of a
    /// compound one
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stmt {
    Block { body: Vec<Node>, handlers: Vec<Handler> },
    Assign { target: String, expr: Span },
    /// `cond` includes its parentheses
    If { cond: Span, then: Box<Node>, otherwise: Option<Box<Node>> },
    While { label: Option<String>, cond: Span, body: Box<Node> },
    ForSelect { label: Option<String>, select: Span, into: Vec<String>, body: Box<Node> },
    /// A statement for the server: a singleton SELECT, DML, EXECUTE
    /// PROCEDURE. `text` leaves out the INTO or RETURNING_VALUES clause.
    Sql { text: Span, into: Vec<String> },
    /// `EXECUTE STATEMENT <text> [INTO …]`, the text an expression
    ExecuteStatement { text: Span, into: Vec<String> },
    Suspend,
    Exit,
    Leave(Option<String>),
    Continue(Option<String>),
    /// `EXCEPTION name [message]`; without a name, the one being handled
    Raise { name: Option<String>, message: Option<Span> },
    Return(Span),
    /// Has no effect on a debugged run, and is said to be skipped
    Skip(String),
    Unsupported(String),
}

/// `WHEN … DO`: what a block does about an error in it
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Handler {
    pub any: bool,
    /// The exceptions named
    pub exceptions: Vec<String>,
    pub body: Node,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Variable {
    pub name: String,
    /// As a CAST takes it: without NOT NULL or a collation
    pub type_text: String,
    pub default: Option<Span>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Body {
    pub variables: Vec<Variable>,
    pub block: Node,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub at: usize,
    pub message: String,
}

impl Body {
    /// The statements the debugger cannot run, and why
    pub fn unsupported(&self) -> Vec<(Span, String)> {
        fn walk(node: &Node, out: &mut Vec<(Span, String)>) {
            match &node.stmt {
                Stmt::Unsupported(what) => out.push((node.span.clone(), what.clone())),
                Stmt::Block { body, handlers } => {
                    body.iter().for_each(|n| walk(n, out));
                    handlers.iter().for_each(|h| walk(&h.body, out));
                }
                Stmt::If { then, otherwise, .. } => {
                    walk(then, out);
                    if let Some(o) = otherwise {
                        walk(o, out);
                    }
                }
                Stmt::While { body, .. } | Stmt::ForSelect { body, .. } => walk(body, out),
                _ => {}
            }
        }
        let mut out = Vec::new();
        walk(&self.block, &mut out);
        out
    }
}

struct Tok {
    kind: TokenKind,
    range: Span,
    /// A word in upper case; anything else as written
    text: String,
}

/// The code of a text: no space, no comments
fn code(text: &str) -> Vec<Tok> {
    tokenize(text)
        .into_iter()
        .filter(|t| !t.kind.is_trivia())
        .map(|t| {
            let raw = &text[t.range.clone()];
            Tok { kind: t.kind, text: if t.kind == TokenKind::Word { raw.to_uppercase() } else { raw.to_string() }, range: t.range }
        })
        .collect()
}

/// A name as the catalog has it: upper case unless quoted. A parameter's
/// colon is dropped.
fn name_of(tok: &Tok) -> Option<String> {
    let text = tok.text.strip_prefix(':').unwrap_or(&tok.text);
    match tok.kind {
        TokenKind::Word => Some(text.to_string()),
        TokenKind::Param if text != "?" && !text.is_empty() => Some(text.to_uppercase()),
        TokenKind::Quoted => Some(text.trim_matches('"').replace("\"\"", "\"")),
        _ => None,
    }
}

struct Parser<'a> {
    source: &'a str,
    toks: Vec<Tok>,
    pos: usize,
}

type Parsed<T> = Result<T, ParseError>;

impl Parser<'_> {
    fn at(&self) -> usize {
        self.toks.get(self.pos).map_or(self.source.len(), |t| t.range.start)
    }

    fn error<T>(&self, message: impl Into<String>) -> Parsed<T> {
        Err(ParseError { at: self.at(), message: message.into() })
    }

    fn is(&self, ahead: usize, kind: TokenKind, text: &str) -> bool {
        self.toks.get(self.pos + ahead).is_some_and(|t| t.kind == kind && t.text == text)
    }

    fn word(&self, ahead: usize, word: &str) -> bool {
        self.is(ahead, TokenKind::Word, word)
    }

    fn punct(&self, ahead: usize, text: &str) -> bool {
        self.is(ahead, TokenKind::Punct, text)
    }

    fn expect_word(&mut self, word: &str) -> Parsed<()> {
        if !self.word(0, word) {
            return self.error(format!("{word} expected"));
        }
        self.pos += 1;
        Ok(())
    }

    /// The last character of the token before the current one
    fn before(&self) -> usize {
        self.toks[self.pos - 1].range.end
    }

    /// `( … )` from where the parser is: its span, parentheses included
    fn parens(&mut self) -> Parsed<Span> {
        if !self.punct(0, "(") {
            return self.error("( expected");
        }
        let start = self.at();
        let mut depth = 0usize;
        while let Some(t) = self.toks.get(self.pos) {
            self.pos += 1;
            if t.kind == TokenKind::Punct && t.text == "(" {
                depth += 1;
            } else if t.kind == TokenKind::Punct && t.text == ")" {
                depth -= 1;
                if depth == 0 {
                    return Ok(start..self.before());
                }
            }
        }
        self.error(") expected")
    }

    /// Where each of `words` is between two tokens, outside parentheses
    fn all(&self, from: usize, to: usize, words: &[&str]) -> Vec<usize> {
        let mut depth = 0usize;
        let mut found = Vec::new();
        for i in from..to {
            let t = &self.toks[i];
            match (t.kind, t.text.as_str()) {
                (TokenKind::Punct, "(") => depth += 1,
                (TokenKind::Punct, ")") => depth = depth.saturating_sub(1),
                (TokenKind::Word, w) if depth == 0 && words.contains(&w) => found.push(i),
                _ => {}
            }
        }
        found
    }

    /// The first of them
    fn find(&self, from: usize, to: usize, words: &[&str]) -> Option<usize> {
        self.all(from, to, words).first().copied()
    }

    /// Where the statement that starts here ends: its `;`
    fn semicolon(&self) -> Parsed<usize> {
        (self.pos..self.toks.len())
            .find(|&i| self.toks[i].kind == TokenKind::Punct && self.toks[i].text == ";")
            .map_or_else(|| self.error("; expected"), Ok)
    }

    /// The text between two tokens, the second left out
    fn between(&self, from: usize, to: usize) -> Span {
        if from >= to {
            let at = self.toks.get(from).map_or(self.source.len(), |t| t.range.start);
            return at..at;
        }
        self.toks[from].range.start..self.toks[to - 1].range.end
    }

    /// Names with commas between them, perhaps in parentheses
    fn names(&self, from: usize, to: usize) -> Parsed<Vec<String>> {
        let mut names = Vec::new();
        for t in &self.toks[from..to] {
            if t.kind == TokenKind::Punct && matches!(t.text.as_str(), "," | "(" | ")") {
                continue;
            }
            match name_of(t) {
                Some(name) => names.push(name),
                None => return Err(ParseError { at: t.range.start, message: "A variable's name expected".into() }),
            }
        }
        Ok(names)
    }

    fn declaration(&mut self, variables: &mut Vec<Variable>) -> Parsed<()> {
        self.pos += 1;
        if self.word(0, "VARIABLE") {
            self.pos += 1;
        }
        if self.word(0, "PROCEDURE") || self.word(0, "FUNCTION") {
            return self.error("A routine that declares routines of its own cannot be debugged");
        }
        let Some(name) = self.toks.get(self.pos).and_then(name_of) else { return self.error("A variable's name expected") };
        self.pos += 1;
        let end = self.semicolon()?;
        // A cursor is declared like a variable, and is none
        if self.find(self.pos, end, &["CURSOR"]).is_none() {
            let default = (self.pos..end).find(|&i| {
                let t = &self.toks[i];
                (t.kind == TokenKind::Punct && t.text == "=") || (t.kind == TokenKind::Word && t.text == "DEFAULT")
            });
            let type_end = self.find(self.pos, default.unwrap_or(end), &["NOT", "COLLATE"]).unwrap_or(default.unwrap_or(end));
            let type_text = self.source[self.between(self.pos, type_end)].trim().to_string();
            variables.push(Variable { name, type_text, default: default.map(|d| self.between(d + 1, end)) });
        }
        self.pos = end + 1;
        Ok(())
    }

    fn block(&mut self) -> Parsed<Node> {
        let start = self.at();
        self.expect_word("BEGIN")?;
        let span = start..self.before();
        let mut body = Vec::new();
        let mut handlers = Vec::new();
        loop {
            if self.pos >= self.toks.len() {
                return self.error("END expected");
            }
            if self.word(0, "END") {
                self.pos += 1;
                if self.punct(0, ";") {
                    self.pos += 1;
                }
                break;
            }
            if self.word(0, "WHEN") {
                let Some(does) = self.find(self.pos, self.toks.len(), &["DO"]) else { return self.error("DO expected") };
                let conditions = &self.toks[self.pos + 1..does];
                let any = conditions.iter().any(|t| t.kind == TokenKind::Word && t.text == "ANY");
                let exceptions = conditions
                    .windows(2)
                    .filter(|w| w[0].kind == TokenKind::Word && w[0].text == "EXCEPTION")
                    .filter_map(|w| name_of(&w[1]))
                    .collect();
                self.pos = does + 1;
                handlers.push(Handler { any, exceptions, body: self.statement()? });
            } else {
                body.push(self.statement()?);
            }
        }
        Ok(Node { stmt: Stmt::Block { body, handlers }, span })
    }

    fn statement(&mut self) -> Parsed<Node> {
        let start = self.at();
        let mut label = None;
        if self.punct(1, ":") && (self.word(2, "WHILE") || self.word(2, "FOR")) {
            label = self.toks.get(self.pos).and_then(name_of);
            self.pos += 2;
        }
        let Some(first) = self.toks.get(self.pos) else { return self.error("A statement expected") };
        let word = if first.kind == TokenKind::Word { first.text.clone() } else { String::new() };
        match word.as_str() {
            "BEGIN" => return self.block(),
            "IF" => {
                self.pos += 1;
                let cond = self.parens()?;
                self.expect_word("THEN")?;
                let span = start..self.before();
                let then = Box::new(self.statement()?);
                let otherwise = if self.word(0, "ELSE") {
                    self.pos += 1;
                    Some(Box::new(self.statement()?))
                } else {
                    None
                };
                return Ok(Node { stmt: Stmt::If { cond, then, otherwise }, span });
            }
            "WHILE" => {
                self.pos += 1;
                let cond = self.parens()?;
                self.expect_word("DO")?;
                let span = start..self.before();
                let body = Box::new(self.statement()?);
                return Ok(Node { stmt: Stmt::While { label, cond, body }, span });
            }
            "FOR" | "IN" => {
                let Some(does) = self.find(self.pos, self.toks.len(), &["DO"]) else { return self.error("DO expected") };
                let span = start..self.toks[does].range.end;
                let select = word == "FOR" && (self.word(1, "SELECT") || self.word(1, "WITH"));
                let into = self.find(self.pos, does, &["INTO"]);
                let cursor = self.find(into.unwrap_or(self.pos), does, &["AS"]);
                let head = (self.pos, into, cursor);
                self.pos = does + 1;
                let body = Box::new(self.statement()?);
                let stmt = match head {
                    _ if word == "IN" => Stmt::Unsupported("A block in a transaction of its own (IN AUTONOMOUS TRANSACTION)".into()),
                    _ if !select => Stmt::Unsupported("A loop over EXECUTE STATEMENT".into()),
                    (_, _, Some(_)) => Stmt::Unsupported("A loop with a named cursor (AS CURSOR)".into()),
                    (_, None, _) => return Err(ParseError { at: start, message: "INTO expected".into() }),
                    (from, Some(into), None) => {
                        Stmt::ForSelect { label, select: self.between(from + 1, into), into: self.names(into + 1, does)?, body }
                    }
                };
                return Ok(Node { stmt, span });
            }
            _ => {}
        }

        // The rest end at their `;`
        let end = self.semicolon()?;
        let from = self.pos;
        let span = start..self.toks[end].range.end;
        let rest = |p: &Self, skip: usize| p.between(from + skip, end);
        let label_at = |p: &Self| p.toks.get(from + 1).filter(|_| from + 1 < end).and_then(name_of);
        let assigns = self.punct(1, "=") && matches!(first.kind, TokenKind::Word | TokenKind::Param | TokenKind::Quoted);
        let stmt = match word.as_str() {
            _ if assigns => Stmt::Assign { target: name_of(first).unwrap_or_default(), expr: rest(self, 2) },
            "SUSPEND" => Stmt::Suspend,
            "EXIT" => Stmt::Exit,
            "LEAVE" | "BREAK" => Stmt::Leave(label_at(self)),
            "CONTINUE" => Stmt::Continue(label_at(self)),
            "RETURN" => Stmt::Return(rest(self, 1)),
            "POST_EVENT" => Stmt::Skip("The event is not posted".into()),
            "OPEN" | "FETCH" | "CLOSE" => Stmt::Unsupported("A cursor opened and fetched by hand".into()),
            "EXCEPTION" => {
                let name = label_at(self);
                let message = (name.is_some() && from + 2 < end).then(|| rest(self, 2));
                if self.find(from, end, &["USING"]).is_some() {
                    Stmt::Unsupported("An exception with parameters (USING)".into())
                } else {
                    Stmt::Raise { name, message }
                }
            }
            "EXECUTE" if self.word(1, "STATEMENT") => {
                let into = self.find(from, end, &["INTO"]);
                let stop = into.unwrap_or(end);
                // The text in parentheses and then more in parentheses: parameters
                let with_parameters = (from + 2..stop).any(|i| {
                    let (a, b) = (&self.toks[i], self.toks.get(i + 1));
                    a.text == ")" && b.is_some_and(|b| b.kind == TokenKind::Punct && b.text == "(" && i + 1 < stop)
                });
                if with_parameters || self.find(from + 2, stop, &["ON", "WITH", "ROLE", "PASSWORD", "USER"]).is_some() {
                    Stmt::Unsupported("EXECUTE STATEMENT with parameters or options".into())
                } else {
                    Stmt::ExecuteStatement {
                        text: self.between(from + 2, stop),
                        into: into.map_or(Ok(Vec::new()), |i| self.names(i + 1, end))?,
                    }
                }
            }
            "EXECUTE" if self.word(1, "PROCEDURE") => match self.find(from, end, &["RETURNING_VALUES"]) {
                Some(r) => Stmt::Sql { text: self.between(from, r), into: self.names(r + 1, end)? },
                None => Stmt::Sql { text: rest(self, 0), into: Vec::new() },
            },
            "EXECUTE" => Stmt::Unsupported("EXECUTE BLOCK inside a routine".into()),
            _ => {
                // INSERT INTO and MERGE INTO name a table; only a SELECT's
                // INTO, or one after RETURNING, names variables
                let selects = matches!(word.as_str(), "SELECT" | "WITH");
                let after = if selects { Some(from) } else { self.find(from, end, &["RETURNING"]) };
                let into = after.and_then(|a| self.all(a, end, &["INTO"]).last().copied());
                match into {
                    Some(i) => Stmt::Sql { text: self.between(from, i), into: self.names(i + 1, end)? },
                    None => Stmt::Sql { text: rest(self, 0), into: Vec::new() },
                }
            }
        };
        self.pos = end + 1;
        Ok(Node { stmt, span })
    }
}

/// Parses a routine's source as the catalog keeps it: its declarations and
/// its block, without the header
pub fn parse(source: &str) -> Result<Body, ParseError> {
    let mut parser = Parser { source, toks: code(source), pos: 0 };
    let mut variables = Vec::new();
    while parser.word(0, "DECLARE") {
        parser.declaration(&mut variables)?;
    }
    if !parser.word(0, "BEGIN") {
        return parser.error("BEGIN expected");
    }
    let block = parser.block()?;
    if parser.pos < parser.toks.len() {
        return parser.error("Nothing expected after the routine's END");
    }
    Ok(Body { variables, block })
}

/// A piece of the source with its variables turned into parameters
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bound {
    pub sql: String,
    /// The variable of each `?`, in order
    pub names: Vec<String>,
}

/// Rewrites `text`, replacing each variable with what `placeholder` gives
/// for it (`None`: not a variable). In SQL (`colon_only`) a variable is
/// `:NAME`; in an expression a bare name is one too, but for a subquery's,
/// where a bare name is a column's.
pub fn bind(text: &str, colon_only: bool, placeholder: impl Fn(&str) -> Option<String>) -> Bound {
    let tokens = tokenize(text);
    let codes: Vec<usize> = (0..tokens.len()).filter(|&i| !tokens[i].kind.is_trivia()).collect();
    let mut sql = String::with_capacity(text.len());
    let mut names = Vec::new();
    // Whether each open parenthesis holds a subquery
    let mut inside: Vec<bool> = Vec::new();
    for (n, &i) in codes.iter().enumerate() {
        let t = &tokens[i];
        let raw = &text[t.range.clone()];
        // What is not code goes as it is, before the token after it
        let gap_from = if n == 0 { 0 } else { tokens[codes[n - 1]].range.end };
        sql.push_str(&text[gap_from..t.range.start]);
        let neighbour = |k: Option<usize>| k.and_then(|k| codes.get(k)).map(|&j| &text[tokens[j].range.clone()]);
        let next = neighbour(Some(n + 1));
        let previous = neighbour(n.checked_sub(1));
        let bare = !colon_only && !inside.iter().any(|&subquery| subquery);
        let name = match t.kind {
            TokenKind::Param if raw != "?" => Some(raw[1..].to_uppercase()),
            TokenKind::Word if bare && previous != Some(".") && !matches!(next, Some("(" | ".")) => Some(raw.to_uppercase()),
            _ => None,
        };
        match name.and_then(|name| placeholder(&name).map(|p| (name, p))) {
            Some((name, p)) => {
                sql.push_str(&p);
                names.push(name);
            }
            None => sql.push_str(raw),
        }
        if t.kind == TokenKind::Punct && raw == "(" {
            inside.push(next.is_some_and(|w| w.eq_ignore_ascii_case("select") || w.eq_ignore_ascii_case("with")));
        } else if t.kind == TokenKind::Punct && raw == ")" {
            inside.pop();
        }
    }
    let tail = codes.last().map_or(0, |&i| tokens[i].range.end);
    sql.push_str(&text[tail..]);
    Bound { sql, names }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(source: &str) -> Vec<Node> {
        match parse(source).unwrap().block.stmt {
            Stmt::Block { body, .. } => body,
            other => panic!("{other:?}"),
        }
    }

    fn text<'a>(source: &'a str, span: &Span) -> &'a str {
        &source[span.clone()]
    }

    #[test]
    fn declarations_keep_the_type_a_cast_takes() {
        let source = "declare variable i integer = 0;\n\
                      DECLARE NOME VARCHAR(40) CHARACTER SET UTF8 NOT NULL COLLATE UNICODE_CI DEFAULT 'x';\n\
                      declare d type of column t.c;\n\
                      declare c cursor for (select 1 from rdb$database);\n\
                      begin end";
        let parsed = parse(source).unwrap();
        let seen: Vec<(&str, &str, Option<&str>)> =
            parsed.variables.iter().map(|v| (v.name.as_str(), v.type_text.as_str(), v.default.as_ref().map(|d| text(source, d)))).collect();
        assert_eq!(
            seen,
            [("I", "integer", Some("0")), ("NOME", "VARCHAR(40) CHARACTER SET UTF8", Some("'x'")), ("D", "type of column t.c", None)]
        );
    }

    #[test]
    fn flow_is_taken_apart_and_the_rest_left_as_text() {
        let source = "begin\n  i = 0;\n  if (i < 2 and (i > 0)) then x = 1; else begin x = 2; end\n  \
                      outer: while (i < 3) do begin i = i + 1; if (i = 2) then leave outer; continue; end\n  \
                      for select a, b from t where c = :i into :a, b do suspend;\n  exit;\nend";
        let nodes = body(source);
        assert_eq!(nodes.len(), 5);
        assert!(matches!(&nodes[0].stmt, Stmt::Assign { target, expr } if target == "I" && text(source, expr) == "0"));
        assert_eq!(text(source, &nodes[0].span), "i = 0;");
        let Stmt::If { cond, then, otherwise } = &nodes[1].stmt else { panic!() };
        assert_eq!(text(source, cond), "(i < 2 and (i > 0))");
        assert_eq!(text(source, &nodes[1].span), "if (i < 2 and (i > 0)) then");
        assert!(matches!(then.stmt, Stmt::Assign { .. }) && matches!(otherwise.as_ref().unwrap().stmt, Stmt::Block { .. }));
        let Stmt::While { label, body, .. } = &nodes[2].stmt else { panic!() };
        assert_eq!(label.as_deref(), Some("OUTER"));
        let Stmt::Block { body, .. } = &body.stmt else { panic!() };
        let Stmt::If { then, .. } = &body[1].stmt else { panic!() };
        assert_eq!(then.stmt, Stmt::Leave(Some("OUTER".into())));
        assert_eq!(body[2].stmt, Stmt::Continue(None));
        let Stmt::ForSelect { select, into, body, .. } = &nodes[3].stmt else { panic!() };
        assert_eq!(text(source, select), "select a, b from t where c = :i");
        assert_eq!(into, &["A", "B"]);
        assert_eq!(body.stmt, Stmt::Suspend);
        assert_eq!(nodes[4].stmt, Stmt::Exit);
    }

    #[test]
    fn sql_knows_which_into_names_variables() {
        let source = "begin\n  select count(*) from (select 1 from t into_x) into :n;\n  insert into t (a) values (:n);\n  \
                      update t set a = 1 where b = :n returning a into :n;\n  merge into t using u on (1 = 1) when matched then update set a = 1;\n  \
                      execute procedure p(:n, 2) returning_values (:a, :b);\n  execute procedure q;\n  \
                      execute statement 'select 1 from rdb$database' into :n;\n  execute statement ('x') (a := 1);\nend";
        let nodes = body(source);
        let sql = |i: usize| match &nodes[i].stmt {
            Stmt::Sql { text: t, into } | Stmt::ExecuteStatement { text: t, into } => (text(source, t), into.join(",")),
            other => panic!("{other:?}"),
        };
        assert_eq!(sql(0), ("select count(*) from (select 1 from t into_x)", "N".to_string()));
        assert_eq!(sql(1), ("insert into t (a) values (:n)", String::new()));
        assert_eq!(sql(2), ("update t set a = 1 where b = :n returning a", "N".to_string()));
        assert_eq!(sql(3).1, "");
        assert_eq!(sql(4), ("execute procedure p(:n, 2)", "A,B".to_string()));
        assert_eq!(sql(5), ("execute procedure q", String::new()));
        assert_eq!(sql(6), ("'select 1 from rdb$database'", "N".to_string()));
        assert!(matches!(nodes[7].stmt, Stmt::Unsupported(_)));
    }

    #[test]
    fn handlers_and_what_cannot_run_are_kept() {
        let source = "begin\n  begin\n    exception e_x 'no ' || :n;\n    exception;\n  when exception e_x, exception e_y do n = 1;\n  when any do begin exit; end\n  end\n  \
                      open c;\n  in autonomous transaction do begin n = 2; end\n  for execute statement 'x' into :n do n = 3;\n  \
                      for select 1 from t as cursor c do n = 4;\n  post_event 'x';\n  return n + 1;\nend";
        let parsed = parse(source).unwrap();
        let Stmt::Block { body, .. } = &parsed.block.stmt else { panic!() };
        let Stmt::Block { body: inner, handlers } = &body[0].stmt else { panic!() };
        assert!(matches!(&inner[0].stmt, Stmt::Raise { name: Some(n), message: Some(m) } if n == "E_X" && text(source, m) == "'no ' || :n"));
        assert_eq!(inner[1].stmt, Stmt::Raise { name: None, message: None });
        assert_eq!(handlers.len(), 2);
        assert_eq!((handlers[0].any, handlers[0].exceptions.as_slice()), (false, &["E_X".to_string(), "E_Y".to_string()][..]));
        assert!(handlers[1].any);
        assert_eq!(parsed.unsupported().len(), 4);
        assert!(matches!(body[5].stmt, Stmt::Skip(_)));
        assert!(matches!(&body[6].stmt, Stmt::Return(e) if text(source, e) == "n + 1"));
    }

    #[test]
    fn what_is_not_psql_says_where() {
        assert_eq!(parse("begin i = 1 end").unwrap_err().message, "; expected");
        assert_eq!(parse("begin if i then exit; end").unwrap_err().at, 9);
        assert!(parse("declare procedure p as begin end begin end").is_err());
        assert!(parse("begin end garbage").is_err());
        assert!(parse("begin").is_err());
    }

    #[test]
    fn variables_become_parameters() {
        let vars = |name: &str| ["N", "NOME"].contains(&name).then(|| format!("<{name}>"));
        let expr = bind("n + coalesce(nome, 'n') + t.n + n(1) + (select max(n) from t where x = :n) /* n */", false, vars);
        assert_eq!(expr.sql, "<N> + coalesce(<NOME>, 'n') + t.n + n(1) + (select max(n) from t where x = <N>) /* n */");
        assert_eq!(expr.names, ["N", "NOME", "N"]);
        let sql = bind("select n from t where a = :N and b = :other and c = ?", true, vars);
        assert_eq!(sql.sql, "select n from t where a = <N> and b = :other and c = ?");
        assert_eq!(sql.names, ["N"]);
    }
}
