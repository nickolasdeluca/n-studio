//! The statements that would create an object again, from what the catalog
//! holds of it.
//!
//! Sources come out as they were written: the engine keeps the text of a
//! view's query, a PSQL body, a default, a CHECK and a computed column, and
//! nothing here reformats it. What is generated is the frame around them,
//! in upper case with four-space indents.

use fb_core::ServerInfo;

use crate::ident::{literal, quote};
use crate::object::{Domain, Exception, Generator, Role};
use crate::routine::{Package, Param, Routine, RoutineKind, Udf, UdfArgument};
use crate::table::{Column, Constraint, ConstraintKind, Index, Storage, Table, Trigger, View};

/// What of the database a declaration depends on
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Context {
    /// The database's dialect
    pub dialect: u32,
    /// The database's default character set, which declarations leave out
    pub charset: String,
    /// The server's major version
    pub major: u32,
}

impl Context {
    pub fn of(info: &ServerInfo) -> Self {
        Self { dialect: info.dialect, charset: info.charset.clone(), major: info.major }
    }

    fn name(&self, name: &str) -> String {
        quote(name, self.dialect)
    }

    fn names(&self, names: &[String]) -> String {
        names.iter().map(|n| self.name(n)).collect::<Vec<_>>().join(", ")
    }
}

/// One statement, without its terminator
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Statement {
    pub sql: String,
    /// It has a PSQL body, whose semicolons a script must not split at
    pub psql: bool,
    /// It adds a foreign key, which needs the table it references: in a
    /// script of several tables it goes after them all
    pub foreign_key: bool,
}

impl Statement {
    fn plain(sql: impl Into<String>) -> Self {
        Self { sql: sql.into(), psql: false, foreign_key: false }
    }

    fn psql(sql: impl Into<String>) -> Self {
        Self { sql: sql.into(), psql: true, foreign_key: false }
    }
}

/// The statements as a script: each ended with `;`, and those with a PSQL
/// body ended with `^` between `SET TERM`s. One-line statements of a kind
/// (a table's constraints, its comments) stay together; a blank line
/// parts the rest.
pub fn script(statements: &[Statement]) -> String {
    // What a run of like statements starts with: ALTER TABLE, COMMENT ON
    let kind = |s: &Statement| -> Option<String> {
        let one_line = !s.psql && !s.sql.trim_end().contains('\n');
        one_line.then(|| s.sql.split_whitespace().take(2).collect::<Vec<_>>().join(" "))
    };
    let mut out = String::new();
    let mut in_psql = false;
    let mut last: Option<String> = None;
    for statement in statements {
        let this = kind(statement);
        if !out.is_empty() && (this.is_none() || this != last || statement.psql != in_psql) {
            out.push('\n');
        }
        if statement.psql != in_psql {
            out.push_str(if statement.psql { "SET TERM ^ ;\n\n" } else { "SET TERM ; ^\n\n" });
            in_psql = statement.psql;
        }
        out.push_str(statement.sql.trim_end());
        out.push_str(if in_psql { "^\n" } else { ";\n" });
        last = this;
    }
    if in_psql {
        out.push_str("\nSET TERM ; ^\n");
    }
    out
}

/// A source as the catalog keeps it: without the blank lines around it,
/// with its own indentation
fn source(text: &str) -> &str {
    let text = text.trim_end();
    // Whole leading lines of blanks go; the first real line keeps its indent
    let start = text.find(|c: char| !c.is_whitespace()).unwrap_or(0);
    let line = text[..start].rfind('\n').map_or(0, |i| i + 1);
    &text[line..]
}

/// A PSQL body from `AS` on. Triggers are stored with their `AS`,
/// procedures and functions without.
fn body(text: Option<&str>) -> String {
    let Some(text) = text.map(source).filter(|t| !t.is_empty()) else {
        return "AS\nBEGIN\n    /* The source is not in the database. */\nEND".into();
    };
    let first = text.trim_start();
    let has_as = first.len() >= 2
        && first[..2].eq_ignore_ascii_case("AS")
        && first[2..].chars().next().is_none_or(char::is_whitespace);
    if has_as {
        first.to_string()
    } else {
        format!("AS\n{text}")
    }
}

fn comment(statements: &mut Vec<Statement>, on: &str, description: Option<&str>) {
    if let Some(text) = description.map(str::trim).filter(|t| !t.is_empty()) {
        statements.push(Statement::plain(format!("COMMENT ON {on} IS {}", literal(text))));
    }
}

pub fn table(t: &Table, ctx: &Context) -> Vec<Statement> {
    let name = ctx.name(&t.name);
    let width = t.columns.iter().map(|c| ctx.name(&c.name).chars().count()).max().unwrap_or(0);
    let columns: Vec<String> = t.columns.iter().map(|c| format!("    {}", column(c, width, ctx))).collect();
    let (create, after) = match &t.storage {
        Storage::Persistent => (format!("CREATE TABLE {name}"), ""),
        Storage::External(file) => (format!("CREATE TABLE {name} EXTERNAL FILE {}", literal(file)), ""),
        Storage::TemporaryPreserve => (format!("CREATE GLOBAL TEMPORARY TABLE {name}"), "\nON COMMIT PRESERVE ROWS"),
        Storage::TemporaryDelete => (format!("CREATE GLOBAL TEMPORARY TABLE {name}"), "\nON COMMIT DELETE ROWS"),
    };
    let mut statements = vec![Statement::plain(format!("{create} (\n{}\n){after}", columns.join(",\n")))];

    statements.extend(t.constraints.iter().map(|c| constraint(&name, c, ctx)));
    for i in t.indices.iter().filter(|i| i.constraint.is_none()) {
        statements.extend(index(i, ctx));
    }
    comment(&mut statements, &format!("TABLE {name}"), t.description.as_deref());
    for c in &t.columns {
        comment(&mut statements, &format!("COLUMN {name}.{}", ctx.name(&c.name)), c.description.as_deref());
    }
    // An index behind a constraint is declared by it, but described apart
    for i in t.indices.iter().filter(|i| i.constraint.is_some()) {
        comment(&mut statements, &format!("INDEX {}", ctx.name(&i.name)), i.description.as_deref());
    }
    statements
}

/// A column's line in CREATE TABLE, its name padded to `width`
fn column(c: &Column, width: usize, ctx: &Context) -> String {
    let name = ctx.name(&c.name);
    let pad = " ".repeat(width.saturating_sub(name.chars().count()));
    if let Some(expression) = &c.computed {
        return format!("{name}{pad}  COMPUTED BY {}", parenthesized(expression));
    }
    let mut line = match &c.domain {
        Some(domain) => format!("{name}{pad}  {}", ctx.name(domain)),
        None => format!("{name}{pad}  {}", c.data_type.declaration(ctx.dialect, &ctx.charset)),
    };
    if let Some(identity) = &c.identity {
        line.push_str(if identity.always {
            " GENERATED ALWAYS AS IDENTITY"
        } else {
            " GENERATED BY DEFAULT AS IDENTITY"
        });
        // Firebird 3 starts at 0 unless told, later versions at 1
        let unset = if ctx.major >= 4 { 1 } else { 0 };
        let mut options = Vec::new();
        if let Some(start) = identity.start.filter(|s| *s != unset) {
            options.push(format!("START WITH {start}"));
        }
        if let Some(increment) = identity.increment.filter(|i| *i != 1 && ctx.major >= 4) {
            options.push(format!("INCREMENT BY {increment}"));
        }
        if !options.is_empty() {
            line.push_str(&format!(" ({})", options.join(" ")));
        }
    }
    if let Some(default) = &c.default {
        line.push_str(&format!(" {}", default.trim()));
    }
    // A key column is flagged NOT NULL itself even when only its domain was
    // declared so; saying it twice is not what anyone wrote
    if c.not_null && !(c.domain.is_some() && c.domain_not_null) {
        line.push_str(" NOT NULL");
    }
    if let Some(collation) = &c.data_type.collation {
        line.push_str(&format!(" COLLATE {collation}"));
    }
    line
}

/// Computed columns and expression indices keep their parentheses, but
/// databases old enough have some without
fn parenthesized(expression: &str) -> String {
    let expression = expression.trim();
    if expression.starts_with('(') && expression.ends_with(')') {
        expression.to_string()
    } else {
        format!("({expression})")
    }
}

fn constraint(table: &str, c: &Constraint, ctx: &Context) -> Statement {
    // INTEG_<n> is the engine's name for a constraint declared without one
    let named = if c.name.starts_with("INTEG_") { String::new() } else { format!("CONSTRAINT {} ", ctx.name(&c.name)) };
    let columns = ctx.names(&c.columns);
    let mut sql = format!("ALTER TABLE {table} ADD {named}");
    match c.kind {
        ConstraintKind::PrimaryKey => sql.push_str(&format!("PRIMARY KEY ({columns})")),
        ConstraintKind::Unique => sql.push_str(&format!("UNIQUE ({columns})")),
        ConstraintKind::ForeignKey => {
            sql.push_str(&format!("FOREIGN KEY ({columns})"));
            if let Some(r) = &c.references {
                sql.push_str(&format!(" REFERENCES {} ({})", ctx.name(&r.table), ctx.names(&r.columns)));
                if let Some(rule) = &r.on_delete {
                    sql.push_str(&format!(" ON DELETE {rule}"));
                }
                if let Some(rule) = &r.on_update {
                    sql.push_str(&format!(" ON UPDATE {rule}"));
                }
            }
        }
        ConstraintKind::Check => sql.push_str(c.check.as_deref().unwrap_or("CHECK (1 = 1)")),
    }
    // A key's index takes the constraint's name unless given its own
    if let Some(index) = c.index.as_deref().filter(|i| c.kind != ConstraintKind::Check && !i.starts_with("RDB$")) {
        if index != c.name || c.index_descending {
            let order = if c.index_descending { "DESCENDING " } else { "" };
            sql.push_str(&format!(" USING {order}INDEX {}", ctx.name(index)));
        }
    }
    Statement { foreign_key: c.kind == ConstraintKind::ForeignKey, ..Statement::plain(sql) }
}

pub fn index(i: &Index, ctx: &Context) -> Vec<Statement> {
    let name = ctx.name(&i.name);
    let unique = if i.unique { "UNIQUE " } else { "" };
    let order = if i.descending { "DESCENDING " } else { "" };
    let on = match &i.expression {
        Some(expression) => format!("COMPUTED BY {}", parenthesized(expression)),
        None => format!("({})", ctx.names(&i.columns)),
    };
    let mut sql = format!("CREATE {unique}{order}INDEX {name} ON {} {on}", ctx.name(&i.table));
    if let Some(condition) = &i.condition {
        sql.push_str(&format!("\n    {}", condition.trim()));
    }
    let mut statements = vec![Statement::plain(sql)];
    if i.inactive {
        statements.push(Statement::plain(format!("ALTER INDEX {name} INACTIVE")));
    }
    comment(&mut statements, &format!("INDEX {name}"), i.description.as_deref());
    statements
}

pub fn view(v: &View, ctx: &Context) -> Vec<Statement> {
    let name = ctx.name(&v.name);
    let columns: Vec<String> = v.columns.iter().map(|c| format!("    {}", ctx.name(&c.name))).collect();
    let query = v.source.as_deref().map(source).unwrap_or("/* The source is not in the database. */");
    let mut statements =
        vec![Statement::plain(format!("CREATE OR ALTER VIEW {name} (\n{})\nAS\n{query}", columns.join(",\n")))];
    comment(&mut statements, &format!("VIEW {name}"), v.description.as_deref());
    for c in &v.columns {
        comment(&mut statements, &format!("COLUMN {name}.{}", ctx.name(&c.name)), c.description.as_deref());
    }
    statements
}

pub fn trigger(t: &Trigger, ctx: &Context) -> Vec<Statement> {
    let name = ctx.name(&t.name);
    let on = t.table.as_ref().map(|table| format!(" FOR {}", ctx.name(table))).unwrap_or_default();
    let active = if t.inactive { "INACTIVE" } else { "ACTIVE" };
    let mut statements = vec![Statement::psql(format!(
        "CREATE OR ALTER TRIGGER {name}{on}\n{active} {} POSITION {}\n{}",
        t.event(),
        t.position,
        body(t.source.as_deref())
    ))];
    comment(&mut statements, &format!("TRIGGER {name}"), t.description.as_deref());
    statements
}

/// A parameter as it is declared
fn param(p: &Param, ctx: &Context) -> String {
    let mut text = if p.name.is_empty() { String::new() } else { format!("{} ", ctx.name(&p.name)) };
    match (&p.type_of_column, &p.domain) {
        (Some((table, column)), _) => {
            text.push_str(&format!("TYPE OF COLUMN {}.{}", ctx.name(table), ctx.name(column)));
        }
        (None, Some(domain)) if p.type_of => text.push_str(&format!("TYPE OF {}", ctx.name(domain))),
        (None, Some(domain)) => text.push_str(&ctx.name(domain)),
        (None, None) => text.push_str(&p.data_type.declaration(ctx.dialect, &ctx.charset)),
    }
    if p.not_null {
        text.push_str(" NOT NULL");
    }
    if let Some(collation) = &p.data_type.collation {
        text.push_str(&format!(" COLLATE {collation}"));
    }
    if let Some(default) = &p.default {
        text.push_str(&format!(" {}", default.trim()));
    }
    text
}

fn params(list: &[Param], ctx: &Context) -> String {
    let lines: Vec<String> = list.iter().map(|p| format!("    {}", param(p, ctx))).collect();
    format!("(\n{})", lines.join(",\n"))
}

pub fn routine(r: &Routine, ctx: &Context) -> Vec<Statement> {
    let name = ctx.name(&r.name);
    let keyword = match r.kind {
        RoutineKind::Procedure => "PROCEDURE",
        RoutineKind::Function => "FUNCTION",
    };
    let mut sql = format!("CREATE OR ALTER {keyword} {name}");
    if !r.inputs.is_empty() {
        sql.push_str(&format!(" {}", params(&r.inputs, ctx)));
    }
    match r.kind {
        RoutineKind::Procedure if !r.outputs.is_empty() => {
            sql.push_str(&format!("\nRETURNS {}", params(&r.outputs, ctx)));
        }
        RoutineKind::Function => {
            if let Some(result) = r.outputs.first() {
                sql.push_str(&format!("\nRETURNS {}", param(result, ctx)));
            }
            if r.deterministic {
                sql.push_str(" DETERMINISTIC");
            }
        }
        RoutineKind::Procedure => {}
    }
    match &r.external {
        Some((engine, entry)) => {
            let entry = entry.as_deref().map(|e| format!(" {}", literal(e))).unwrap_or_default();
            sql.push_str(&format!("\nEXTERNAL NAME{entry}\nENGINE {engine}"));
        }
        None => sql.push_str(&format!("\n{}", body(r.source.as_deref()))),
    }
    let mut statements = vec![Statement::psql(sql)];
    comment(&mut statements, &format!("{keyword} {name}"), r.description.as_deref());
    if r.kind == RoutineKind::Procedure {
        for p in r.inputs.iter().chain(&r.outputs) {
            comment(&mut statements, &format!("PARAMETER {name}.{}", ctx.name(&p.name)), p.description.as_deref());
        }
    }
    statements
}

pub fn package(p: &Package, ctx: &Context) -> Vec<Statement> {
    let name = ctx.name(&p.name);
    let mut statements =
        vec![Statement::psql(format!("CREATE OR ALTER PACKAGE {name}\n{}", body(p.header.as_deref())))];
    if p.body.is_some() {
        statements.push(Statement::psql(format!("RECREATE PACKAGE BODY {name}\n{}", body(p.body.as_deref()))));
    }
    comment(&mut statements, &format!("PACKAGE {name}"), p.description.as_deref());
    statements
}

pub fn domain(d: &Domain, ctx: &Context) -> Vec<Statement> {
    let name = ctx.name(&d.name);
    let mut sql = format!("CREATE DOMAIN {name} AS {}", d.data_type.declaration(ctx.dialect, &ctx.charset));
    if let Some(default) = &d.default {
        sql.push_str(&format!("\n    {}", default.trim()));
    }
    if d.not_null {
        sql.push_str("\n    NOT NULL");
    }
    if let Some(check) = &d.check {
        sql.push_str(&format!("\n    {}", check.trim()));
    }
    if let Some(collation) = &d.data_type.collation {
        sql.push_str(&format!("\n    COLLATE {collation}"));
    }
    let mut statements = vec![Statement::plain(sql)];
    comment(&mut statements, &format!("DOMAIN {name}"), d.description.as_deref());
    statements
}

/// With its value, when it was read and is not where a new one starts
pub fn generator(g: &Generator, ctx: &Context) -> Vec<Statement> {
    let name = ctx.name(&g.name);
    let mut sql = format!("CREATE SEQUENCE {name}");
    if let Some(increment) = g.increment.filter(|i| *i != 1 && ctx.major >= 4) {
        sql.push_str(&format!(" INCREMENT BY {increment}"));
    }
    let mut statements = vec![Statement::plain(sql)];
    if let Some(value) = g.value.filter(|v| *v != 0) {
        // Unlike RESTART WITH, this means the same on every version
        statements.push(Statement::plain(format!("SET GENERATOR {name} TO {value}")));
    }
    comment(&mut statements, &format!("SEQUENCE {name}"), g.description.as_deref());
    statements
}

pub fn exception(e: &Exception, ctx: &Context) -> Vec<Statement> {
    let name = ctx.name(&e.name);
    let mut statements =
        vec![Statement::plain(format!("CREATE EXCEPTION {name} {}", literal(e.message.as_deref().unwrap_or(""))))];
    comment(&mut statements, &format!("EXCEPTION {name}"), e.description.as_deref());
    statements
}

pub fn role(r: &Role, ctx: &Context) -> Vec<Statement> {
    let name = ctx.name(&r.name);
    let mut statements = vec![Statement::plain(format!("CREATE ROLE {name}"))];
    comment(&mut statements, &format!("ROLE {name}"), r.description.as_deref());
    statements
}

pub fn udf(u: &Udf, ctx: &Context) -> Vec<Statement> {
    let name = ctx.name(&u.name);
    let argument = |a: &UdfArgument, result: bool| -> String {
        let how = match a.mechanism.abs() {
            0 if result => " BY VALUE",
            2 => " BY DESCRIPTOR",
            4 => " BY SCALAR_ARRAY",
            5 if !result => " NULL",
            _ => "",
        };
        let free = if a.mechanism < 0 { " FREE_IT" } else { "" };
        format!("{}{how}{free}", a.data_type.declaration(ctx.dialect, &ctx.charset))
    };
    let mut sql = format!("DECLARE EXTERNAL FUNCTION {name}");
    if !u.arguments.is_empty() {
        let lines: Vec<String> = u.arguments.iter().map(|a| format!("    {}", argument(a, false))).collect();
        sql.push_str(&format!("\n{}", lines.join(",\n")));
    }
    match (u.returns_parameter, &u.returns) {
        (Some(position), _) => sql.push_str(&format!("\nRETURNS PARAMETER {position}")),
        (None, Some(result)) => sql.push_str(&format!("\nRETURNS {}", argument(result, true))),
        (None, None) => {}
    }
    sql.push_str(&format!(
        "\nENTRY_POINT {} MODULE_NAME {}",
        literal(u.entry_point.as_deref().unwrap_or("")),
        literal(u.module.as_deref().unwrap_or(""))
    ));
    let mut statements = vec![Statement::plain(sql)];
    comment(&mut statements, &format!("EXTERNAL FUNCTION {name}"), u.description.as_deref());
    statements
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::table::{Identity, Reference};
    use crate::types::DataType;

    fn ctx() -> Context {
        Context { dialect: 3, charset: "UTF8".into(), major: 4 }
    }

    fn integer() -> DataType {
        DataType { code: 8, ..Default::default() }
    }

    fn varchar(n: i64, charset: &str) -> DataType {
        DataType { code: 37, char_length: n, charset: Some(charset.into()), ..Default::default() }
    }

    fn col(name: &str, data_type: DataType) -> Column {
        Column { name: name.into(), data_type, ..Default::default() }
    }

    #[test]
    fn a_table_with_everything_on_it() {
        let lines = Table {
            name: "PEDIDO_ITEM".into(),
            storage: Storage::Persistent,
            columns: vec![
                Column {
                    not_null: true,
                    identity: Some(Identity { always: false, start: Some(1), increment: Some(1) }),
                    ..col("ID", integer())
                },
                Column { domain: Some("D_ID".into()), not_null: true, ..col("PEDIDO", integer()) },
                Column { default: Some("DEFAULT 'N'".into()), not_null: true, ..col("Obs", varchar(60, "WIN1252")) },
                Column {
                    data_type: DataType { collation: Some("UNICODE_CI".into()), ..varchar(40, "UTF8") },
                    description: Some("It's the name".into()),
                    ..col("NOME", integer())
                },
                Column { computed: Some("(QTD * PRECO)".into()), ..col("TOTAL", integer()) },
            ],
            constraints: vec![
                Constraint {
                    name: "PK_PEDIDO_ITEM".into(),
                    kind: ConstraintKind::PrimaryKey,
                    columns: vec!["ID".into()],
                    index: Some("PK_PEDIDO_ITEM".into()),
                    index_descending: false,
                    references: None,
                    check: None,
                },
                Constraint {
                    name: "INTEG_12".into(),
                    kind: ConstraintKind::Unique,
                    columns: vec!["PEDIDO".into(), "Obs".into()],
                    index: Some("RDB$12".into()),
                    index_descending: false,
                    references: None,
                    check: None,
                },
                Constraint {
                    name: "FK_ITEM_PEDIDO".into(),
                    kind: ConstraintKind::ForeignKey,
                    columns: vec!["PEDIDO".into()],
                    index: Some("IDX_ITEM_PEDIDO".into()),
                    index_descending: false,
                    references: Some(Reference {
                        table: "PEDIDO".into(),
                        columns: vec!["ID".into()],
                        on_update: None,
                        on_delete: Some("CASCADE".into()),
                    }),
                    check: None,
                },
                Constraint {
                    name: "CK_ITEM".into(),
                    kind: ConstraintKind::Check,
                    columns: vec![],
                    index: None,
                    index_descending: false,
                    references: None,
                    check: Some("CHECK (QTD > 0)".into()),
                },
            ],
            indices: vec![
                Index {
                    name: "IDX_ITEM_NOME".into(),
                    table: "PEDIDO_ITEM".into(),
                    unique: true,
                    descending: true,
                    inactive: true,
                    columns: vec!["NOME".into()],
                    expression: None,
                    condition: None,
                    statistics: None,
                    constraint: None,
                    description: None,
                },
                Index {
                    name: "IDX_ITEM_UPPER".into(),
                    table: "PEDIDO_ITEM".into(),
                    unique: false,
                    descending: false,
                    inactive: false,
                    columns: vec![],
                    expression: Some("(upper(NOME))".into()),
                    condition: Some("WHERE QTD > 0".into()),
                    statistics: None,
                    constraint: None,
                    description: None,
                },
                Index {
                    name: "PK_PEDIDO_ITEM".into(),
                    table: "PEDIDO_ITEM".into(),
                    unique: true,
                    descending: false,
                    inactive: false,
                    columns: vec!["ID".into()],
                    expression: None,
                    condition: None,
                    statistics: None,
                    constraint: Some(ConstraintKind::PrimaryKey),
                    description: None,
                },
            ],
            description: Some("Order lines".into()),
        };
        let expected = "\
CREATE TABLE PEDIDO_ITEM (
    ID      INTEGER GENERATED BY DEFAULT AS IDENTITY NOT NULL,
    PEDIDO  D_ID NOT NULL,
    \"Obs\"   VARCHAR(60) CHARACTER SET WIN1252 DEFAULT 'N' NOT NULL,
    NOME    VARCHAR(40) COLLATE UNICODE_CI,
    TOTAL   COMPUTED BY (QTD * PRECO)
);

ALTER TABLE PEDIDO_ITEM ADD CONSTRAINT PK_PEDIDO_ITEM PRIMARY KEY (ID);
ALTER TABLE PEDIDO_ITEM ADD UNIQUE (PEDIDO, \"Obs\");
ALTER TABLE PEDIDO_ITEM ADD CONSTRAINT FK_ITEM_PEDIDO FOREIGN KEY (PEDIDO) REFERENCES PEDIDO (ID) ON DELETE CASCADE USING INDEX IDX_ITEM_PEDIDO;
ALTER TABLE PEDIDO_ITEM ADD CONSTRAINT CK_ITEM CHECK (QTD > 0);

CREATE UNIQUE DESCENDING INDEX IDX_ITEM_NOME ON PEDIDO_ITEM (NOME);

ALTER INDEX IDX_ITEM_NOME INACTIVE;

CREATE INDEX IDX_ITEM_UPPER ON PEDIDO_ITEM COMPUTED BY (upper(NOME))
    WHERE QTD > 0;

COMMENT ON TABLE PEDIDO_ITEM IS 'Order lines';
COMMENT ON COLUMN PEDIDO_ITEM.NOME IS 'It''s the name';
";
        assert_eq!(script(&table(&lines, &ctx())), expected);
    }

    #[test]
    fn temporary_and_external_tables_say_so() {
        let mut t = Table { name: "T".into(), columns: vec![col("A", integer())], ..Default::default() };
        t.storage = Storage::TemporaryDelete;
        assert_eq!(
            script(&table(&t, &ctx())),
            "CREATE GLOBAL TEMPORARY TABLE T (\n    A  INTEGER\n)\nON COMMIT DELETE ROWS;\n"
        );
        t.storage = Storage::External("/data/t.dat".into());
        assert_eq!(script(&table(&t, &ctx())), "CREATE TABLE T EXTERNAL FILE '/data/t.dat' (\n    A  INTEGER\n);\n");
    }

    #[test]
    fn psql_goes_between_set_terms() {
        let procedure = Routine {
            name: "P_FIXTURE".into(),
            kind: RoutineKind::Procedure,
            inputs: vec![
                Param { name: "P_N".into(), data_type: integer(), ..Default::default() },
                Param {
                    name: "P_TXT".into(),
                    data_type: varchar(10, "UTF8"),
                    not_null: true,
                    default: Some("= 'x'".into()),
                    ..Default::default()
                },
            ],
            outputs: vec![Param {
                name: "R_ID".into(),
                domain: Some("D_ID".into()),
                type_of: true,
                data_type: integer(),
                ..Default::default()
            }],
            source: Some("\n  \nBEGIN\n  R_ID = P_N;\n  SUSPEND;\nEND  \n".into()),
            deterministic: false,
            external: None,
            description: Some("A fixture".into()),
        };
        let trigger = Trigger {
            name: "TIPOS_BI".into(),
            table: Some("TIPOS".into()),
            type_code: 1,
            position: 0,
            inactive: false,
            source: Some("AS\nBEGIN\n  NEW.ID = 1;\nEND".into()),
            description: None,
        };
        let mut statements = routine(&procedure, &ctx());
        statements.extend(self::trigger(&trigger, &ctx()));
        let expected = "\
SET TERM ^ ;

CREATE OR ALTER PROCEDURE P_FIXTURE (
    P_N INTEGER,
    P_TXT VARCHAR(10) NOT NULL = 'x')
RETURNS (
    R_ID TYPE OF D_ID)
AS
BEGIN
  R_ID = P_N;
  SUSPEND;
END^

SET TERM ; ^

COMMENT ON PROCEDURE P_FIXTURE IS 'A fixture';

SET TERM ^ ;

CREATE OR ALTER TRIGGER TIPOS_BI FOR TIPOS
ACTIVE BEFORE INSERT POSITION 0
AS
BEGIN
  NEW.ID = 1;
END^

SET TERM ; ^
";
        assert_eq!(script(&statements), expected);
    }

    #[test]
    fn a_function_returns_one_value() {
        let function = Routine {
            name: "F_DOBRO".into(),
            kind: RoutineKind::Function,
            inputs: vec![Param { name: "N".into(), data_type: integer(), ..Default::default() }],
            outputs: vec![Param { data_type: integer(), ..Default::default() }],
            source: Some("BEGIN\n  RETURN N * 2;\nEND".into()),
            deterministic: true,
            external: None,
            description: None,
        };
        let expected = "\
SET TERM ^ ;

CREATE OR ALTER FUNCTION F_DOBRO (
    N INTEGER)
RETURNS INTEGER DETERMINISTIC
AS
BEGIN
  RETURN N * 2;
END^

SET TERM ; ^
";
        assert_eq!(script(&routine(&function, &ctx())), expected);
    }

    #[test]
    fn the_small_kinds() {
        let d = Domain {
            name: "D_FLAG".into(),
            data_type: DataType { code: 14, char_length: 1, charset: Some("UTF8".into()), ..Default::default() },
            default: Some("DEFAULT 'N'".into()),
            check: Some("CHECK (VALUE IN ('S','N'))".into()),
            not_null: true,
            description: None,
        };
        assert_eq!(
            script(&domain(&d, &ctx())),
            "CREATE DOMAIN D_FLAG AS CHAR(1)\n    DEFAULT 'N'\n    NOT NULL\n    CHECK (VALUE IN ('S','N'));\n"
        );
        let g = Generator {
            name: "GEN_ID".into(),
            value: Some(42),
            initial: Some(1),
            increment: Some(1),
            description: None,
        };
        assert_eq!(script(&generator(&g, &ctx())), "CREATE SEQUENCE GEN_ID;\n\nSET GENERATOR GEN_ID TO 42;\n");
        let e = Exception { name: "E_X".into(), number: 1, message: Some("can't".into()), description: None };
        assert_eq!(script(&exception(&e, &ctx())), "CREATE EXCEPTION E_X 'can''t';\n");
        let v = View {
            name: "V_TIPOS".into(),
            columns: vec![col("ID", integer()), col("NOME", integer())],
            source: Some("\n  SELECT ID, NOME FROM TIPOS".into()),
            description: None,
        };
        assert_eq!(
            script(&view(&v, &ctx())),
            "CREATE OR ALTER VIEW V_TIPOS (\n    ID,\n    NOME)\nAS\n  SELECT ID, NOME FROM TIPOS;\n"
        );
        let u = Udf {
            name: "STRLEN".into(),
            module: Some("ib_udf".into()),
            entry_point: Some("IB_UDF_strlen".into()),
            arguments: vec![UdfArgument {
                data_type: DataType { code: 40, char_length: 255, ..Default::default() },
                mechanism: 1,
            }],
            returns: Some(UdfArgument { data_type: integer(), mechanism: 0 }),
            returns_parameter: None,
            description: None,
        };
        assert_eq!(
            script(&udf(&u, &ctx())),
            "DECLARE EXTERNAL FUNCTION STRLEN\n    CSTRING(255)\nRETURNS INTEGER BY VALUE\nENTRY_POINT 'IB_UDF_strlen' MODULE_NAME 'ib_udf';\n"
        );
    }
}
