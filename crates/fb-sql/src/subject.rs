//! What a statement is about, from its first words alone: the operation and
//! the object it touches, as a script's explorer groups them. No parser: a
//! statement not recognised is `Other`, never an error.

use serde::Serialize;

use crate::lexer::{tokenize, Token, TokenKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Operation {
    Create,
    Alter,
    CreateOrAlter,
    Recreate,
    Drop,
    Comment,
    Grant,
    Revoke,
    Insert,
    Update,
    UpdateOrInsert,
    Delete,
    Merge,
    Select,
    Execute,
    Commit,
    Rollback,
    SetGenerator,
    Connect,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ObjectKind {
    Table,
    View,
    Procedure,
    Function,
    Package,
    Trigger,
    Domain,
    Generator,
    Exception,
    Index,
    Role,
    User,
    Udf,
    Database,
    /// `EXECUTE BLOCK`: a Firebird block, a group of its own
    Block,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Subject {
    pub operation: Operation,
    pub kind: Option<ObjectKind>,
    /// As the catalog stores it: as written when quoted, upper case if not
    pub name: Option<String>,
    /// Below the object: the column of `COMMENT ON COLUMN T.C`, the table of
    /// `CREATE INDEX … ON T`, the table of `CREATE TRIGGER … FOR T`
    pub member: Option<String>,
}

impl Subject {
    fn of(operation: Operation) -> Self {
        Self { operation, kind: None, name: None, member: None }
    }
}

/// What `sql`, one statement, is about
pub fn classify(sql: &str) -> Subject {
    let tokens = tokenize(sql);
    let tokens: Vec<&Token> = tokens.iter().filter(|t| !t.kind.is_trivia()).collect();
    Words { sql, tokens: &tokens, at: 0 }.subject()
}

/// The statement's tokens, read from the front
struct Words<'a> {
    sql: &'a str,
    tokens: &'a [&'a Token],
    at: usize,
}

impl Words<'_> {
    /// The next token as an upper-case keyword, if it is a word
    fn peek(&self) -> Option<String> {
        self.peek_at(0)
    }

    fn peek_at(&self, ahead: usize) -> Option<String> {
        let t = self.tokens.get(self.at + ahead)?;
        (t.kind == TokenKind::Word).then(|| self.sql[t.range.clone()].to_ascii_uppercase())
    }

    /// Takes the next token if it is the keyword `word`
    fn eat(&mut self, word: &str) -> bool {
        if self.peek().as_deref() == Some(word) {
            self.at += 1;
            true
        } else {
            false
        }
    }

    fn next_word(&mut self) -> Option<String> {
        let word = self.peek()?;
        self.at += 1;
        Some(word)
    }

    /// An identifier, as the catalog stores it
    fn name(&mut self) -> Option<String> {
        let t = self.tokens.get(self.at)?;
        let text = &self.sql[t.range.clone()];
        let name = match t.kind {
            TokenKind::Word => text.to_ascii_uppercase(),
            TokenKind::Quoted if !t.unterminated => text[1..text.len() - 1].replace("\"\"", "\""),
            _ => return None,
        };
        self.at += 1;
        Some(name)
    }

    fn punct(&mut self, c: &str) -> bool {
        match self.tokens.get(self.at) {
            Some(t) if t.kind == TokenKind::Punct && &self.sql[t.range.clone()] == c => {
                self.at += 1;
                true
            }
            _ => false,
        }
    }

    /// `name` or `name.member`
    fn dotted(&mut self) -> (Option<String>, Option<String>) {
        let name = self.name();
        let member = if name.is_some() && self.punct(".") { self.name() } else { None };
        (name, member)
    }

    /// Moves to the first `word` outside parentheses, past it; false if
    /// there is none
    fn seek(&mut self, word: &str) -> bool {
        let mut depth = 0i32;
        while let Some(t) = self.tokens.get(self.at) {
            self.at += 1;
            let text = &self.sql[t.range.clone()];
            match t.kind {
                TokenKind::Punct if text == "(" => depth += 1,
                TokenKind::Punct if text == ")" => depth -= 1,
                TokenKind::Word if depth == 0 && text.eq_ignore_ascii_case(word) => return true,
                _ => {}
            }
        }
        false
    }

    fn subject(&mut self) -> Subject {
        let Some(first) = self.next_word() else {
            return Subject::of(Operation::Other);
        };
        match first.as_str() {
            "CREATE" => {
                if self.eat("OR") && self.eat("ALTER") {
                    self.object(Operation::CreateOrAlter)
                } else {
                    self.object(Operation::Create)
                }
            }
            "ALTER" => self.object(Operation::Alter),
            "RECREATE" => self.object(Operation::Recreate),
            "DROP" => self.object(Operation::Drop),
            "DECLARE" => self.object(Operation::Create),
            "COMMENT" if self.eat("ON") => self.comment(),
            "DESCRIBE" => self.describe(),
            "GRANT" => self.grant(Operation::Grant),
            "REVOKE" => self.grant(Operation::Revoke),
            "INSERT" if self.eat("INTO") => self.table(Operation::Insert),
            "UPDATE" if self.peek().as_deref() == Some("OR") && self.peek_at(1).as_deref() == Some("INSERT") => {
                self.at += 2;
                self.eat("INTO");
                self.table(Operation::UpdateOrInsert)
            }
            "UPDATE" => self.table(Operation::Update),
            "DELETE" if self.eat("FROM") => self.table(Operation::Delete),
            "MERGE" if self.eat("INTO") => self.table(Operation::Merge),
            "SELECT" | "WITH" => Subject::of(Operation::Select),
            "EXECUTE" if self.eat("PROCEDURE") => self.named(Operation::Execute, ObjectKind::Procedure),
            "EXECUTE" if self.eat("BLOCK") => {
                Subject { kind: Some(ObjectKind::Block), ..Subject::of(Operation::Execute) }
            }
            "COMMIT" => Subject::of(Operation::Commit),
            "ROLLBACK" => Subject::of(Operation::Rollback),
            "SET" if self.eat("GENERATOR") => self.named(Operation::SetGenerator, ObjectKind::Generator),
            "CONNECT" => Subject::of(Operation::Connect),
            _ => Subject::of(Operation::Other),
        }
    }

    fn named(&mut self, operation: Operation, kind: ObjectKind) -> Subject {
        Subject { operation, kind: Some(kind), name: self.name(), member: None }
    }

    fn table(&mut self, operation: Operation) -> Subject {
        self.named(operation, ObjectKind::Table)
    }

    /// The object kind next, in the words DDL names it by
    fn kind(&mut self) -> Option<ObjectKind> {
        let word = self.next_word()?;
        Some(match word.as_str() {
            "TABLE" => ObjectKind::Table,
            "GLOBAL" if self.eat("TEMPORARY") && self.eat("TABLE") => ObjectKind::Table,
            "VIEW" => ObjectKind::View,
            "PROCEDURE" => ObjectKind::Procedure,
            "FUNCTION" => ObjectKind::Function,
            "PACKAGE" => {
                self.eat("BODY");
                ObjectKind::Package
            }
            "TRIGGER" => ObjectKind::Trigger,
            "DOMAIN" => ObjectKind::Domain,
            "GENERATOR" | "SEQUENCE" => ObjectKind::Generator,
            "EXCEPTION" => ObjectKind::Exception,
            "UNIQUE" | "ASC" | "ASCENDING" | "DESC" | "DESCENDING" => return self.kind(),
            "INDEX" => ObjectKind::Index,
            "ROLE" => ObjectKind::Role,
            "USER" => ObjectKind::User,
            "EXTERNAL" if self.eat("FUNCTION") => ObjectKind::Udf,
            "DATABASE" | "SCHEMA" => ObjectKind::Database,
            _ => return None,
        })
    }

    /// `<kind> <name>` after a DDL verb
    fn object(&mut self, operation: Operation) -> Subject {
        let Some(kind) = self.kind() else {
            return Subject::of(operation);
        };
        let mut subject = Subject { operation, kind: Some(kind), name: None, member: None };
        if kind == ObjectKind::Database {
            return subject;
        }
        subject.name = self.name();
        match kind {
            ObjectKind::Index if self.eat("ON") => subject.member = self.name(),
            ObjectKind::Trigger if self.eat("FOR") => subject.member = self.name(),
            ObjectKind::Trigger => subject.member = self.trigger_table(),
            _ => {}
        }
        subject
    }

    /// The table of Firebird 3's `ACTIVE BEFORE INSERT ON t`: looked for
    /// only in the header, the body may have `ON`s of its own
    fn trigger_table(&mut self) -> Option<String> {
        while let Some(word) = self.next_word() {
            match word.as_str() {
                "AS" | "BEGIN" | "DECLARE" | "EXTERNAL" | "SQL" => return None,
                "ON" => {
                    // A database trigger: ON CONNECT, ON TRANSACTION COMMIT
                    const EVENTS: &[&str] = &["CONNECT", "DISCONNECT", "TRANSACTION"];
                    return self.peek().filter(|w| EVENTS.contains(&w.as_str())).map_or_else(|| self.name(), |_| None);
                }
                _ => {}
            }
        }
        None
    }

    /// After `COMMENT ON`
    fn comment(&mut self) -> Subject {
        let mut subject = Subject::of(Operation::Comment);
        match self.peek().as_deref() {
            Some("COLUMN") => {
                self.at += 1;
                subject.kind = Some(ObjectKind::Table);
                (subject.name, subject.member) = self.dotted();
            }
            Some("PARAMETER") => {
                // Of a procedure or, from Firebird 3, a function: not said
                self.at += 1;
                subject.kind = Some(ObjectKind::Procedure);
                (subject.name, subject.member) = self.dotted();
            }
            _ => {
                subject.kind = self.kind();
                if matches!(subject.kind, Some(ObjectKind::Procedure | ObjectKind::Function)) && self.eat("PARAMETER") {
                    // Firebird 3: COMMENT ON PROCEDURE PARAMETER p.x
                    (subject.name, subject.member) = self.dotted();
                } else if subject.kind.is_some_and(|k| k != ObjectKind::Database) {
                    subject.name = self.name();
                }
            }
        }
        subject
    }

    /// IBExpert's `DESCRIBE <kind> name 'text'`, and `DESCRIBE FIELD c
    /// TABLE t 'text'` / `DESCRIBE PARAMETER p PROCEDURE x 'text'`
    fn describe(&mut self) -> Subject {
        let mut subject = Subject::of(Operation::Comment);
        match self.peek().as_deref() {
            Some("FIELD" | "PARAMETER") => {
                self.at += 1;
                subject.member = self.name();
                subject.kind = self.kind();
                subject.name = self.name();
            }
            _ => {
                subject.kind = self.kind();
                subject.name = self.name();
            }
        }
        subject
    }

    /// `GRANT … ON [kind] name TO …`, or `GRANT role TO …`
    fn grant(&mut self, operation: Operation) -> Subject {
        let start = self.at;
        if self.seek("ON") {
            let before = self.at;
            let kind = self.kind().unwrap_or_else(|| {
                // `ON T` is a table (or view)
                self.at = before;
                ObjectKind::Table
            });
            return Subject { operation, kind: Some(kind), name: self.name(), member: None };
        }
        self.at = start;
        // REVOKE ADMIN OPTION FOR role FROM …
        if operation == Operation::Revoke && self.eat("ADMIN") {
            self.eat("OPTION");
            self.eat("FOR");
        }
        const NOT_ROLES: &[&str] = &["CREATE", "ALTER", "DROP", "ALL", "DEFAULT"];
        if self.peek().is_some_and(|w| NOT_ROLES.contains(&w.as_str())) {
            return Subject::of(operation);
        }
        self.eat("ROLE");
        self.named(operation, ObjectKind::Role)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ObjectKind::*;
    use Operation::*;

    fn s(sql: &str) -> (Operation, Option<ObjectKind>, Option<&'static str>, Option<&'static str>) {
        let subject = classify(sql);
        let leak = |s: Option<String>| s.map(|s| &*Box::leak(s.into_boxed_str()));
        (subject.operation, subject.kind, leak(subject.name), leak(subject.member))
    }

    #[test]
    fn ddl_names_its_object() {
        assert_eq!(
            s("ALTER PROCEDURE NORMALIZA_TEXTO (ENTRADA VARCHAR(100)) AS BEGIN END"),
            (Alter, Some(Procedure), Some("NORMALIZA_TEXTO"), None)
        );
        assert_eq!(s("create table cliente (id int)"), (Create, Some(Table), Some("CLIENTE"), None));
        assert_eq!(
            s("create or alter trigger t_bi for cliente before insert as begin end"),
            (CreateOrAlter, Some(Trigger), Some("T_BI"), Some("CLIENTE"))
        );
        assert_eq!(
            s("create trigger t_bi active before insert on cliente as begin end"),
            (Create, Some(Trigger), Some("T_BI"), Some("CLIENTE"))
        );
        assert_eq!(
            s("alter trigger t_bi as begin select 1 from a join b on 1 = 1 into :x; end"),
            (Alter, Some(Trigger), Some("T_BI"), None)
        );
        assert_eq!(s("create trigger t_conn on connect as begin end"), (Create, Some(Trigger), Some("T_CONN"), None));
        assert_eq!(s("recreate view v as select 1 from rdb$database"), (Recreate, Some(View), Some("V"), None));
        assert_eq!(s("drop domain d_nome"), (Drop, Some(Domain), Some("D_NOME"), None));
        assert_eq!(s("create global temporary table tmp (id int)"), (Create, Some(Table), Some("TMP"), None));
        assert_eq!(s("create package body pk as begin end"), (Create, Some(Package), Some("PK"), None));
        assert_eq!(s("create sequence g_id"), (Create, Some(Generator), Some("G_ID"), None));
        assert_eq!(s("ALTER SEQUENCE g_id RESTART WITH 10"), (Alter, Some(Generator), Some("G_ID"), None));
        assert_eq!(s("create exception e_x 'boom'"), (Create, Some(Exception), Some("E_X"), None));
        assert_eq!(
            s("declare external function f returns int by value entry_point 'f' module_name 'm'"),
            (Create, Some(Udf), Some("F"), None)
        );
        assert_eq!(s("create database 'x.fdb'"), (Create, Some(Database), None, None));
        assert_eq!(s("create collation c for win1252"), (Create, None, None, None));
    }

    #[test]
    fn indices_name_their_table() {
        assert_eq!(
            s("CREATE UNIQUE DESC INDEX IX_A ON PRODUTO (CODIGO)"),
            (Create, Some(Index), Some("IX_A"), Some("PRODUTO"))
        );
        assert_eq!(
            s("create index ix_b on \"Produto\" (codigo)"),
            (Create, Some(Index), Some("IX_B"), Some("Produto"))
        );
        assert_eq!(s("alter index ix_b inactive"), (Alter, Some(Index), Some("IX_B"), None));
    }

    #[test]
    fn quoted_names_keep_their_case_and_comments_go_before_anything() {
        assert_eq!(
            s("/* header */ -- line\n  alter table \"Mixed \"\"Q\"\"\" add x int"),
            (Alter, Some(Table), Some("Mixed \"Q\""), None)
        );
    }

    #[test]
    fn comments_land_on_their_object() {
        assert_eq!(
            s("COMMENT ON COLUMN PARAMETRO.COLUNA IS 'x'"),
            (Comment, Some(Table), Some("PARAMETRO"), Some("COLUNA"))
        );
        assert_eq!(s("comment on table cliente is 'x'"), (Comment, Some(Table), Some("CLIENTE"), None));
        assert_eq!(s("comment on procedure parameter p.a is 'x'"), (Comment, Some(Procedure), Some("P"), Some("A")));
        assert_eq!(s("comment on database is 'x'"), (Comment, Some(Database), None, None));
        assert_eq!(s("DESCRIBE TABLE CLIENTE 'text'"), (Comment, Some(Table), Some("CLIENTE"), None));
        assert_eq!(
            s("DESCRIBE FIELD NOME TABLE CLIENTE 'text'"),
            (Comment, Some(Table), Some("CLIENTE"), Some("NOME"))
        );
    }

    #[test]
    fn dml_is_filed_under_its_table() {
        assert_eq!(s("insert into cliente values (1)"), (Insert, Some(Table), Some("CLIENTE"), None));
        assert_eq!(s("update cliente set a = 1"), (Update, Some(Table), Some("CLIENTE"), None));
        assert_eq!(
            s("update or insert into cliente (id) values (1) matching (id)"),
            (UpdateOrInsert, Some(Table), Some("CLIENTE"), None)
        );
        assert_eq!(s("delete from cliente"), (Delete, Some(Table), Some("CLIENTE"), None));
        assert_eq!(
            s("merge into cliente c using x on 1=1 when matched then delete"),
            (Merge, Some(Table), Some("CLIENTE"), None)
        );
        assert_eq!(s("select * from cliente"), (Select, None, None, None));
    }

    #[test]
    fn execution_and_transactions() {
        assert_eq!(s("execute procedure p_seed 10"), (Execute, Some(Procedure), Some("P_SEED"), None));
        assert_eq!(s("EXECUTE BLOCK AS BEGIN END"), (Execute, Some(Block), None, None));
        assert_eq!(s("COMMIT WORK"), (Commit, None, None, None));
        assert_eq!(s("rollback"), (Rollback, None, None, None));
        assert_eq!(s("SET GENERATOR G_ID TO 5"), (SetGenerator, Some(Generator), Some("G_ID"), None));
        assert_eq!(s("CONNECT 'x' USER 'y'"), (Connect, None, None, None));
        assert_eq!(s("set sql dialect 3"), (Other, None, None, None));
        assert_eq!(s(""), (Other, None, None, None));
    }

    #[test]
    fn grants_name_what_they_grant_on() {
        assert_eq!(s("GRANT SELECT, UPDATE (A, B) ON CLIENTE TO PUBLIC"), (Grant, Some(Table), Some("CLIENTE"), None));
        assert_eq!(s("grant execute on procedure p to user u"), (Grant, Some(Procedure), Some("P"), None));
        assert_eq!(s("grant usage on sequence g to r"), (Grant, Some(Generator), Some("G"), None));
        assert_eq!(s("revoke all on cliente from u"), (Revoke, Some(Table), Some("CLIENTE"), None));
        assert_eq!(s("grant gerente to u with admin option"), (Grant, Some(Role), Some("GERENTE"), None));
        assert_eq!(s("revoke admin option for gerente from u"), (Revoke, Some(Role), Some("GERENTE"), None));
        assert_eq!(s("grant create table to u"), (Grant, None, None, None));
    }
}
