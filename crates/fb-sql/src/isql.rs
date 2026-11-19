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

/// `CREATE DATABASE`, as the client reads it before it creates the
/// database: what the server is asked through the attach's parameters
#[derive(Clone, PartialEq, Eq)]
pub struct CreateDatabase {
    pub connect: Connect,
    pub page_size: Option<u32>,
    /// `DEFAULT CHARACTER SET`
    pub charset: Option<String>,
    /// `SET NAMES`: the connection's character set
    pub names: Option<String>,
    /// What else it says, which is not applied: secondary files, a
    /// difference file, a default collation
    pub ignored: Vec<String>,
}

impl fmt::Debug for CreateDatabase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CreateDatabase")
            .field("connect", &self.connect)
            .field("page_size", &self.page_size)
            .field("charset", &self.charset)
            .field("names", &self.names)
            .field("ignored", &self.ignored)
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
    /// A database to create, and connect to after
    CreateDatabase(CreateDatabase),
    /// The database connected to, deleted
    DropDatabase,
    /// Where the rows of the selects that follow are written: a file, or
    /// none again
    Output(Option<String>),
    /// What `SHOW` is asked, as written after it
    Show(String),
    /// A command for the system's shell, as written
    Shell(String),
    /// `SET BAIL`: whether a failure stops the script
    SetBail(bool),
    /// `EXIT` commits and ends the script; `QUIT` rolls back and ends it
    Exit,
    Quit,
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
        (Some("CREATE"), Some("DATABASE" | "SCHEMA")) => {
            let Some(database) = value(2) else {
                return Command::Other;
            };
            let mut create = CreateDatabase {
                connect: Connect { database, user: None, password: None, role: None },
                page_size: None,
                charset: None,
                names: None,
                ignored: Vec::new(),
            };
            let mut i = 3;
            // `=` after PAGE_SIZE and LENGTH is optional
            let after_eq = |i: usize| if tokens.get(i).is_some_and(|t| &sql[t.range.clone()] == "=") { i + 1 } else { i };
            while i < tokens.len() {
                let at = word(i);
                match at.as_deref() {
                    Some("USER") => create.connect.user = value(i + 1),
                    Some("PASSWORD") => create.connect.password = value(i + 1),
                    Some("ROLE") => create.connect.role = value(i + 1),
                    Some("PAGE_SIZE") => {
                        i = after_eq(i + 1);
                        create.page_size = value(i).and_then(|n| n.parse().ok());
                        i += 1;
                        continue;
                    }
                    Some("LENGTH") => {
                        i = after_eq(i + 1) + 1;
                        if matches!(word(i).as_deref(), Some("PAGE" | "PAGES")) {
                            i += 1;
                        }
                        continue;
                    }
                    Some("SET") if word(i + 1).as_deref() == Some("NAMES") => {
                        create.names = value(i + 2);
                        i += 3;
                        continue;
                    }
                    Some("DEFAULT") if word(i + 1).as_deref() == Some("CHARACTER") => {
                        create.charset = value(i + 3);
                        i += 4;
                        continue;
                    }
                    Some(other @ ("COLLATION" | "DIFFERENCE" | "FILE")) => {
                        if !create.ignored.iter().any(|w| w == other) {
                            create.ignored.push(other.to_string());
                        }
                    }
                    _ => {}
                }
                i += if matches!(at.as_deref(), Some("USER" | "PASSWORD" | "ROLE")) { 2 } else { 1 };
            }
            Command::CreateDatabase(create)
        }
        (Some("DROP"), Some("DATABASE")) if tokens.len() == 2 => Command::DropDatabase,
        (Some("INPUT"), _) => value(1).map_or(Command::Other, Command::Input),
        (Some("OUTPUT"), _) => Command::Output(value(1)),
        (Some("SHOW"), _) => Command::Show(rest_after(sql, &tokens, 1)),
        (Some("SHELL"), _) => Command::Shell(rest_after(sql, &tokens, 1)),
        (Some("SET"), Some("BAIL")) => Command::SetBail(word(2).as_deref() != Some("OFF")),
        (Some("EXIT"), None) => Command::Exit,
        (Some("QUIT"), None) => Command::Quit,
        _ => Command::Other,
    }
}

/// The text after the `skip` first tokens, trimmed
fn rest_after(sql: &str, tokens: &[&Token], skip: usize) -> String {
    tokens.get(skip).map_or(String::new(), |t| sql[t.range.start..].trim().to_string())
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
    fn create_database_is_read_as_the_client_reads_it() {
        let Command::CreateDatabase(c) = command(
            "CREATE DATABASE 'srv/3050:/data/new.fdb' USER 'SYSDBA' PASSWORD 'pw' PAGE_SIZE = 16384 \
             LENGTH 100 PAGES DEFAULT CHARACTER SET WIN1252 COLLATION WIN_PTBR DIFFERENCE FILE '/d.delta'",
        ) else {
            panic!("a create")
        };
        assert_eq!(c.connect.database, "srv/3050:/data/new.fdb");
        assert_eq!((c.connect.user.as_deref(), c.connect.password.as_deref()), (Some("SYSDBA"), Some("pw")));
        assert_eq!((c.page_size, c.charset.as_deref()), (Some(16384), Some("WIN1252")));
        assert_eq!(c.ignored, ["COLLATION", "DIFFERENCE", "FILE"]);
        assert!(!format!("{c:?}").contains("pw"));
        let Command::CreateDatabase(c) = command("create schema 'x.fdb' page_size 8192 set names 'UTF8'") else {
            panic!()
        };
        assert_eq!((c.page_size, c.names.as_deref(), c.charset), (Some(8192), Some("UTF8"), None));
        assert_eq!(command("DROP DATABASE"), Command::DropDatabase);
        assert_eq!(command("DROP DATABASE x"), Command::Other);
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
        assert_eq!(command("SHOW TABLES"), Command::Show("TABLES".into()));
        assert_eq!(command("show table  Cliente"), Command::Show("table  Cliente".into()));
        assert_eq!(command("OUTPUT 'rows.txt'"), Command::Output(Some("rows.txt".into())));
        assert_eq!(command("OUTPUT"), Command::Output(None));
        assert_eq!(command("SHELL ls -l /tmp"), Command::Shell("ls -l /tmp".into()));
        assert_eq!((command("SET BAIL ON"), command("set bail off")), (Command::SetBail(true), Command::SetBail(false)));
        assert_eq!((command("EXIT"), command("quit")), (Command::Exit, Command::Quit));
        assert_eq!(command("SET NAMES"), Command::Other);
    }
}

/// The catalog select that answers what `SHOW` is asked (`TABLES`,
/// `TABLE name`, `PROCEDURES`, …), its rows what isql would print. `None`
/// for what it is not one of these: isql's other `SHOW`s, which ask for
/// the definitions the explorer shows.
pub fn show_query(what: &str) -> Option<String> {
    let mut words = what.split_whitespace();
    let kind = words.next()?.to_ascii_uppercase();
    let name: Option<String> = words.next().map(|n| match n.strip_prefix('"').and_then(|n| n.strip_suffix('"')) {
        Some(quoted) => quoted.replace("\"\"", "\""),
        None => n.to_ascii_uppercase(),
    });
    let literal = |s: &str| format!("'{}'", s.replace('\'', "''"));
    let user = "coalesce(rdb$system_flag, 0) = 0";
    let list = |column: &str, label: &str, table: &str, filter: &str| {
        format!("select trim({column}) as \"{label}\" from {table} where {filter} order by 1")
    };
    Some(match (kind.as_str(), name) {
        ("TABLE" | "TABLES" | "VIEW" | "VIEWS", Some(name)) => format!(
            "select trim(rf.rdb$field_name) as \"Column\", trim(rf.rdb$field_source) as \"Domain\", \
             iif(coalesce(rf.rdb$null_flag, f.rdb$null_flag, 0) = 1, 'NOT NULL', '') as \"Null\", \
             iif(f.rdb$computed_blr is null, '', 'COMPUTED') as \"Computed\" \
             from rdb$relation_fields rf join rdb$fields f on f.rdb$field_name = rf.rdb$field_source \
             where rf.rdb$relation_name = {} order by rf.rdb$field_position",
            literal(&name)
        ),
        ("TABLE" | "TABLES", None) => {
            list("rdb$relation_name", "Table", "rdb$relations", &format!("rdb$view_blr is null and {user}"))
        }
        ("VIEW" | "VIEWS", None) => {
            list("rdb$relation_name", "View", "rdb$relations", &format!("rdb$view_blr is not null and {user}"))
        }
        ("PROCEDURE" | "PROCEDURES", _) => list("rdb$procedure_name", "Procedure", "rdb$procedures", user),
        ("FUNCTION" | "FUNCTIONS", _) => list("rdb$function_name", "Function", "rdb$functions", user),
        ("PACKAGE" | "PACKAGES", _) => list("rdb$package_name", "Package", "rdb$packages", user),
        ("TRIGGER" | "TRIGGERS", _) => format!(
            "select trim(rdb$trigger_name) as \"Trigger\", trim(rdb$relation_name) as \"Table\", \
             iif(coalesce(rdb$trigger_inactive, 0) = 1, 'INACTIVE', 'ACTIVE') as \"State\" \
             from rdb$triggers where {user} order by 2, 1"
        ),
        ("GENERATOR" | "GENERATORS" | "SEQUENCE" | "SEQUENCES", _) => {
            list("rdb$generator_name", "Generator", "rdb$generators", user)
        }
        ("DOMAIN" | "DOMAINS", _) => list("rdb$field_name", "Domain", "rdb$fields", "rdb$field_name not starting 'RDB$'"),
        ("EXCEPTION" | "EXCEPTIONS", _) => list("rdb$exception_name", "Exception", "rdb$exceptions", user),
        ("ROLE" | "ROLES", _) => list("rdb$role_name", "Role", "rdb$roles", user),
        ("INDEX" | "INDEXES" | "INDICES" | "IND", name) => format!(
            "select trim(rdb$index_name) as \"Index\", trim(rdb$relation_name) as \"Table\", \
             iif(rdb$unique_flag = 1, 'UNIQUE', '') as \"Unique\", \
             iif(coalesce(rdb$index_inactive, 0) = 1, 'INACTIVE', '') as \"State\" \
             from rdb$indices where {user}{} order by 2, 1",
            name.map(|n| format!(" and rdb$relation_name = {}", literal(&n))).unwrap_or_default()
        ),
        ("DATABASE" | "DB", _) => "select mon$database_name as \"Database\", mon$page_size as \"Page size\", \
             mon$ods_major || '.' || mon$ods_minor as \"ODS\", mon$sql_dialect as \"Dialect\", \
             (select trim(rdb$character_set_name) from rdb$database) as \"Character set\" from mon$database"
            .to_string(),
        ("VERSION" | "VER", _) => {
            "select rdb$get_context('SYSTEM', 'ENGINE_VERSION') as \"Server version\" from rdb$database".to_string()
        }
        _ => return None,
    })
}

#[cfg(test)]
mod show_tests {
    use super::show_query;

    #[test]
    fn show_asks_the_catalog() {
        assert!(show_query("TABLES").unwrap().contains("rdb$view_blr is null"));
        let one = show_query("table cliente").unwrap();
        assert!(one.contains("rdb$relation_name = 'CLIENTE'"), "{one}");
        assert!(show_query("TABLE \"Mixed\"").unwrap().contains("= 'Mixed'"));
        assert!(show_query("ind pedidos").unwrap().contains("rdb$relation_name = 'PEDIDOS'"));
        assert!(show_query("procedures").unwrap().contains("rdb$procedures"));
        assert_eq!(show_query("GRANTS"), None);
        assert_eq!(show_query(""), None);
    }
}
