//! The isql commands a script runner follows rather than passes over: the
//! ones that say where to connect and how to run what comes after.

use std::fmt;

use crate::lexer::{tokenize, Token, TokenKind};

#[derive(Clone, PartialEq, Eq)]
pub struct Connect {
    /// As written: `host/port:path`, `host:path`, a path or an alias
    pub database: String,
    pub user: Option<String>,
    pub password: Option<String>,
    pub role: Option<String>,
}

/// Never prints the password: a script's `CONNECT` goes into logs and
/// messages, its password must not
impl fmt::Debug for Connect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Connect")
            .field("database", &self.database)
            .field("user", &self.user)
            .field("password", &self.password.as_ref().map(|_| "…"))
            .field("role", &self.role)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Connect(Connect),
    /// The character set of the next `CONNECT`
    SetNames(String),
    SetAutoDdl(bool),
    SetSqlDialect(u8),
    /// The client library isql loads: nothing to load here, the client is
    /// this program's own
    SetClientLib,
    /// Another script to run from here
    Input(String),
    Other,
}

/// What an isql command (or IBExpert's) says to do
pub fn command(sql: &str) -> Command {
    let tokens = tokenize(sql);
    let tokens: Vec<&Token> = tokens.iter().filter(|t| !t.kind.is_trivia()).collect();
    let word = |i: usize| {
        tokens.get(i).filter(|t| t.kind == TokenKind::Word).map(|t| sql[t.range.clone()].to_ascii_uppercase())
    };
    // A string, a quoted name or a bare word, as isql takes them
    let value = |i: usize| -> Option<String> {
        let t = tokens.get(i)?;
        let text = &sql[t.range.clone()];
        match t.kind {
            TokenKind::String if !t.unterminated && text.starts_with('\'') => {
                Some(text[1..text.len() - 1].replace("''", "'"))
            }
            TokenKind::Quoted if !t.unterminated => Some(text[1..text.len() - 1].replace("\"\"", "\"")),
            TokenKind::Word | TokenKind::Number => Some(text.to_string()),
            _ => None,
        }
    };
    match (word(0).as_deref(), word(1).as_deref()) {
        (Some("CONNECT"), _) => {
            let Some(database) = value(1) else {
                return Command::Other;
            };
            let mut connect = Connect { database, user: None, password: None, role: None };
            let mut i = 2;
            while i < tokens.len() {
                match word(i).as_deref() {
                    Some("USER") => connect.user = value(i + 1),
                    Some("PASSWORD") => connect.password = value(i + 1),
                    Some("ROLE") => connect.role = value(i + 1),
                    _ => {
                        i += 1;
                        continue;
                    }
                }
                i += 2;
            }
            Command::Connect(connect)
        }
        (Some("SET"), Some("NAMES")) => value(2).map_or(Command::Other, Command::SetNames),
        (Some("SET"), Some("AUTODDL" | "AUTO")) => {
            // isql toggles without an argument; a script that bothers to say
            // it nearly always means to turn it on
            Command::SetAutoDdl(word(2).as_deref() != Some("OFF"))
        }
        (Some("SET"), Some("SQL")) if word(2).as_deref() == Some("DIALECT") => {
            value(3).and_then(|n| n.parse().ok()).map_or(Command::Other, Command::SetSqlDialect)
        }
        (Some("SET"), Some("CLIENTLIB")) => Command::SetClientLib,
        (Some("INPUT"), _) => value(1).map_or(Command::Other, Command::Input),
        _ => Command::Other,
    }
}

/// IBExpert's `DESCRIBE`, as the `COMMENT ON` it means: `DESCRIBE <kind>
/// name 'text'`, `DESCRIBE FIELD c TABLE t 'text'` and `DESCRIBE PARAMETER
/// p PROCEDURE x 'text'`. Names are kept as written. `None` for anything
/// else.
pub fn describe_as_comment(sql: &str) -> Option<String> {
    let tokens = tokenize(sql);
    let tokens: Vec<&Token> = tokens.iter().filter(|t| !t.kind.is_trivia()).collect();
    let text = |t: &Token| &sql[t.range.clone()];
    let (first, rest) = tokens.split_first()?;
    let (comment, middle) = rest.split_last()?;
    if !text(first).eq_ignore_ascii_case("DESCRIBE") || comment.kind != TokenKind::String || comment.unterminated {
        return None;
    }
    let is_name = |t: &&Token| matches!(t.kind, TokenKind::Word | TokenKind::Quoted);
    let upper = |t: &Token| text(t).to_ascii_uppercase();
    match middle {
        [what, member, kind, name] if is_name(member) && is_name(name) => {
            let on = match (upper(what).as_str(), upper(kind).as_str()) {
                ("FIELD", "TABLE" | "VIEW") => "COLUMN",
                ("PARAMETER", "PROCEDURE") => "PARAMETER",
                ("PARAMETER", "FUNCTION") => "FUNCTION PARAMETER",
                _ => return None,
            };
            Some(format!("COMMENT ON {on} {}.{} IS {}", text(name), text(member), text(comment)))
        }
        [kinds @ .., name] if !kinds.is_empty() && is_name(name) && kinds.iter().all(|t| t.kind == TokenKind::Word) => {
            let kind: Vec<String> = kinds.iter().map(|t| upper(t)).collect();
            Some(format!("COMMENT ON {} {} IS {}", kind.join(" "), text(name), text(comment)))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connect_takes_its_clauses_in_any_order() {
        let c = command("CONNECT 'server/3050:C:\\data\\x.fdb' USER 'SYSDBA' PASSWORD 'it''s' ROLE r_admin");
        assert_eq!(
            c,
            Command::Connect(Connect {
                database: "server/3050:C:\\data\\x.fdb".into(),
                user: Some("SYSDBA".into()),
                password: Some("it's".into()),
                role: Some("r_admin".into()),
            })
        );
        let c = command("connect \"alias\" role 'R' user sysdba");
        assert_eq!(
            c,
            Command::Connect(Connect {
                database: "alias".into(),
                user: Some("sysdba".into()),
                password: None,
                role: Some("R".into())
            })
        );
        assert_eq!(command("CONNECT"), Command::Other);
    }

    #[test]
    fn a_connect_never_shows_its_password() {
        let Command::Connect(c) = command("CONNECT 'db' USER 'u' PASSWORD 'secret'") else { panic!() };
        assert!(!format!("{c:?}").contains("secret"));
    }

    #[test]
    fn describe_is_a_comment() {
        assert_eq!(
            describe_as_comment("DESCRIBE TABLE CLIENTE 'it''s'").unwrap(),
            "COMMENT ON TABLE CLIENTE IS 'it''s'"
        );
        assert_eq!(
            describe_as_comment("describe field nome table \"Cliente\" 'x'").unwrap(),
            "COMMENT ON COLUMN \"Cliente\".nome IS 'x'"
        );
        assert_eq!(
            describe_as_comment("DESCRIBE PARAMETER A PROCEDURE P 'x'").unwrap(),
            "COMMENT ON PARAMETER P.A IS 'x'"
        );
        assert_eq!(describe_as_comment("DESCRIBE TABLE CLIENTE"), None);
        assert_eq!(describe_as_comment("SHOW TABLE CLIENTE 'x'"), None);
    }

    #[test]
    fn set_commands() {
        assert_eq!(command("SET NAMES WIN1252"), Command::SetNames("WIN1252".into()));
        assert_eq!(command("set autoddl off"), Command::SetAutoDdl(false));
        assert_eq!(command("SET AUTO  ON"), Command::SetAutoDdl(true));
        assert_eq!(command("SET AUTODDL"), Command::SetAutoDdl(true));
        assert_eq!(command("SET SQL DIALECT 1"), Command::SetSqlDialect(1));
        assert_eq!(command("SET CLIENTLIB 'C:\\fbclient.dll'"), Command::SetClientLib);
        assert_eq!(command("INPUT 'other.sql'"), Command::Input("other.sql".into()));
        assert_eq!(command("SHOW TABLES"), Command::Other);
        assert_eq!(command("SET NAMES"), Command::Other);
    }
}
