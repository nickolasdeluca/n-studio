//! The statements that change an object, where [`ddl`](crate::ddl) has the
//! ones that would create it again: describing it, dropping it, and what a
//! new one of its kind starts from.

use crate::ddl::{script, Context, Statement};
use crate::ident::{literal, quote};
use crate::ObjectKind;

/// What DDL calls a kind, in `COMMENT ON` and `DROP`
fn keyword(kind: ObjectKind) -> &'static str {
    match kind {
        ObjectKind::Domain => "DOMAIN",
        ObjectKind::Table => "TABLE",
        ObjectKind::View => "VIEW",
        ObjectKind::Procedure => "PROCEDURE",
        ObjectKind::Function => "FUNCTION",
        ObjectKind::Package => "PACKAGE",
        ObjectKind::Trigger => "TRIGGER",
        // GENERATOR says the same, on every version; this is the standard's
        ObjectKind::Generator => "SEQUENCE",
        ObjectKind::Exception => "EXCEPTION",
        ObjectKind::Udf => "EXTERNAL FUNCTION",
        ObjectKind::Role => "ROLE",
        ObjectKind::Index => "INDEX",
    }
}

/// Sets an object's description; an empty one removes it
pub fn describe(kind: ObjectKind, name: &str, text: &str, ctx: &Context) -> Statement {
    let text = text.trim_end();
    let value = if text.trim().is_empty() { "NULL".to_string() } else { literal(text) };
    Statement::plain(format!("COMMENT ON {} {} IS {value}", keyword(kind), quote(name, ctx.dialect)))
}

pub fn drop(kind: ObjectKind, name: &str, ctx: &Context) -> Statement {
    Statement::plain(format!("DROP {} {}", keyword(kind), quote(name, ctx.dialect)))
}

/// A script that creates a new object of a kind, for its author to rewrite:
/// the frame, with the smallest body the server takes
pub fn template(kind: ObjectKind, ctx: &Context) -> String {
    let statement = match kind {
        ObjectKind::Domain => Statement::plain("CREATE DOMAIN NEW_DOMAIN AS VARCHAR(60)"),
        ObjectKind::Table => Statement::plain(
            "CREATE TABLE NEW_TABLE (\n    ID  INTEGER NOT NULL,\n    CONSTRAINT PK_NEW_TABLE PRIMARY KEY (ID)\n)",
        ),
        ObjectKind::View => {
            Statement::plain("CREATE OR ALTER VIEW NEW_VIEW (\n    ID)\nAS\nSELECT 1 FROM RDB$DATABASE")
        }
        ObjectKind::Procedure => Statement::psql(
            "CREATE OR ALTER PROCEDURE NEW_PROCEDURE (\n    P_ID INTEGER)\nRETURNS (\n    R_ID INTEGER)\nAS\nBEGIN\n  \
             R_ID = P_ID;\n  SUSPEND;\nEND",
        ),
        ObjectKind::Function => Statement::psql(
            "CREATE OR ALTER FUNCTION NEW_FUNCTION (\n    P_ID INTEGER)\nRETURNS INTEGER\nAS\nBEGIN\n  RETURN P_ID;\nEND",
        ),
        ObjectKind::Package => {
            return script(&[
                Statement::psql(
                    "CREATE OR ALTER PACKAGE NEW_PACKAGE\nAS\nBEGIN\n  FUNCTION TWICE (P_N INTEGER) RETURNS INTEGER;\nEND",
                ),
                Statement::psql(
                    "RECREATE PACKAGE BODY NEW_PACKAGE\nAS\nBEGIN\n  FUNCTION TWICE (P_N INTEGER) RETURNS INTEGER\n  AS\n  \
                     BEGIN\n    RETURN P_N * 2;\n  END\nEND",
                ),
            ]);
        }
        ObjectKind::Trigger => Statement::psql(
            "CREATE OR ALTER TRIGGER NEW_TRIGGER FOR TABLE_NAME\nACTIVE BEFORE INSERT POSITION 0\nAS\nBEGIN\n  \
             /* NEW.ID = NEXT VALUE FOR GENERATOR_NAME; */\nEND",
        ),
        ObjectKind::Generator => Statement::plain("CREATE SEQUENCE NEW_GENERATOR"),
        ObjectKind::Exception => Statement::plain("CREATE EXCEPTION NEW_EXCEPTION 'The message'"),
        ObjectKind::Udf => Statement::plain(
            "DECLARE EXTERNAL FUNCTION NEW_UDF\n    INTEGER\nRETURNS INTEGER BY VALUE\nENTRY_POINT 'entry_point' \
             MODULE_NAME 'module'",
        ),
        ObjectKind::Role => Statement::plain("CREATE ROLE NEW_ROLE"),
        ObjectKind::Index => Statement::plain("CREATE INDEX NEW_INDEX ON TABLE_NAME (COLUMN_NAME)"),
    };
    // Dialect 1 has no quoted names to care about; the names above need none
    let _ = ctx;
    script(&[statement])
}

/// A name as someone typed it, as DDL should have it: what SQL would make
/// of it unquoted when it can go unquoted (`nome` is `NOME`), in quotes as
/// typed otherwise. One already in quotes is taken at its word.
pub fn typed(name: &str, ctx: &Context) -> String {
    let name = name.trim();
    if name.len() >= 2 && name.starts_with('"') && name.ends_with('"') {
        return name.to_string();
    }
    let mut chars = name.chars();
    let plain = chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$');
    if plain {
        quote(&name.to_ascii_uppercase(), ctx.dialect)
    } else {
        quote(name, ctx.dialect)
    }
}

/// A typed name as the catalog would keep it: `nome` is `NOME`, `"Nome"`
/// is `Nome`
pub fn stored(name: &str) -> String {
    let name = name.trim();
    if let Some(inner) = name.strip_prefix('"').and_then(|n| n.strip_suffix('"')).filter(|_| name.len() >= 2) {
        return inner.replace("\"\"", "\"");
    }
    let mut chars = name.chars();
    let plain = chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$');
    if plain {
        name.to_ascii_uppercase()
    } else {
        name.to_string()
    }
}

/// Typed names, separated by commas: `id, "Nome"`
pub fn typed_list(names: &str, ctx: &Context) -> String {
    names.split(',').map(str::trim).filter(|n| !n.is_empty()).map(|n| typed(n, ctx)).collect::<Vec<_>>().join(", ")
}

/// A fragment of SQL someone typed, or nothing when they typed nothing
fn given(text: &str) -> Option<&str> {
    let text = text.trim();
    (!text.is_empty()).then_some(text)
}

/// An expression with the keyword before it, unless it was typed with it:
/// `0` and `DEFAULT 0` both give `DEFAULT 0`
fn led_by(keyword: &str, text: &str) -> String {
    let has = text.len() > keyword.len()
        && text[..keyword.len()].eq_ignore_ascii_case(keyword)
        && text[keyword.len()..].starts_with(|c: char| c.is_whitespace() || c == '(');
    if has {
        text.to_string()
    } else {
        format!("{keyword} {text}")
    }
}

/// An expression in parentheses, unless it was typed in them
fn in_parentheses(text: &str) -> String {
    if text.starts_with('(') && text.ends_with(')') {
        text.to_string()
    } else {
        format!("({text})")
    }
}

/// A column to add, as typed
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NewColumn {
    pub name: String,
    /// A type or a domain's name: `VARCHAR(60)`, `D_ID`
    pub data_type: String,
    pub not_null: bool,
    /// With or without `DEFAULT`
    pub default: String,
    /// A computed column's expression; the type is then the expression's
    pub computed: String,
    pub description: String,
}

pub fn add_column(table: &str, column: &NewColumn, ctx: &Context) -> Vec<Statement> {
    let table = quote(table, ctx.dialect);
    let name = typed(&column.name, ctx);
    let mut sql = format!("ALTER TABLE {table} ADD {name} ");
    if let Some(expression) = given(&column.computed) {
        sql.push_str(&format!("COMPUTED BY {}", in_parentheses(expression)));
    } else {
        sql.push_str(column.data_type.trim());
        if let Some(default) = given(&column.default) {
            sql.push_str(&format!(" {}", led_by("DEFAULT", default)));
        }
        if column.not_null {
            sql.push_str(" NOT NULL");
        }
    }
    let mut statements = vec![Statement::plain(sql)];
    if let Some(text) = given(&column.description) {
        statements.push(Statement::plain(format!("COMMENT ON COLUMN {table}.{name} IS {}", literal(text))));
    }
    statements
}

/// What to change of a column; each `None` leaves that as it is
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ColumnChange {
    pub rename: Option<String>,
    pub data_type: Option<String>,
    /// `Some("")` drops the default
    pub default: Option<String>,
    /// Firebird 3 and later
    pub not_null: Option<bool>,
    /// From 1
    pub position: Option<u32>,
    /// `Some("")` removes the description
    pub description: Option<String>,
}

/// The statements for a column's changes, the renaming last so the others
/// can name the column as it is
pub fn alter_column(table: &str, column: &str, change: &ColumnChange, ctx: &Context) -> Vec<Statement> {
    let table = quote(table, ctx.dialect);
    let name = quote(column, ctx.dialect);
    let alter = |what: String| Statement::plain(format!("ALTER TABLE {table} ALTER COLUMN {name} {what}"));
    let mut statements = Vec::new();
    if let Some(data_type) = change.data_type.as_deref().and_then(given) {
        statements.push(alter(format!("TYPE {data_type}")));
    }
    if let Some(default) = &change.default {
        statements.push(match given(default) {
            Some(default) => alter(format!("SET {}", led_by("DEFAULT", default))),
            None => alter("DROP DEFAULT".into()),
        });
    }
    if let Some(not_null) = change.not_null {
        statements.push(alter(if not_null { "SET NOT NULL" } else { "DROP NOT NULL" }.into()));
    }
    if let Some(position) = change.position {
        statements.push(alter(format!("POSITION {position}")));
    }
    if let Some(text) = &change.description {
        let value = given(text).map_or("NULL".to_string(), literal);
        statements.push(Statement::plain(format!("COMMENT ON COLUMN {table}.{name} IS {value}")));
    }
    if let Some(new) = change.rename.as_deref().and_then(given) {
        statements.push(alter(format!("TO {}", typed(new, ctx))));
    }
    statements
}

pub fn drop_column(table: &str, column: &str, ctx: &Context) -> Statement {
    Statement::plain(format!("ALTER TABLE {} DROP {}", quote(table, ctx.dialect), quote(column, ctx.dialect)))
}

/// A constraint to add, as typed
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NewConstraint {
    PrimaryKey { columns: String },
    Unique { columns: String },
    ForeignKey { columns: String, table: String, references: String, on_update: String, on_delete: String },
    /// With or without `CHECK` and its parentheses
    Check { condition: String },
}

/// `name` may be empty: the server then names the constraint
pub fn add_constraint(table: &str, name: &str, constraint: &NewConstraint, ctx: &Context) -> Statement {
    let named = given(name).map_or(String::new(), |n| format!("CONSTRAINT {} ", typed(n, ctx)));
    let what = match constraint {
        NewConstraint::PrimaryKey { columns } => format!("PRIMARY KEY ({})", typed_list(columns, ctx)),
        NewConstraint::Unique { columns } => format!("UNIQUE ({})", typed_list(columns, ctx)),
        NewConstraint::ForeignKey { columns, table, references, on_update, on_delete } => {
            let mut sql = format!("FOREIGN KEY ({}) REFERENCES {}", typed_list(columns, ctx), typed(table, ctx));
            // Without them, the referenced table's primary key
            if given(references).is_some() {
                sql.push_str(&format!(" ({})", typed_list(references, ctx)));
            }
            for (event, rule) in [("DELETE", on_delete), ("UPDATE", on_update)] {
                if let Some(rule) = given(rule).filter(|r| !r.eq_ignore_ascii_case("RESTRICT")) {
                    sql.push_str(&format!(" ON {event} {}", rule.to_ascii_uppercase()));
                }
            }
            sql
        }
        NewConstraint::Check { condition } => {
            let condition = condition.trim();
            if condition.len() > 5 && condition[..5].eq_ignore_ascii_case("CHECK") {
                condition.to_string()
            } else {
                format!("CHECK {}", in_parentheses(condition))
            }
        }
    };
    let mut statement = Statement::plain(format!("ALTER TABLE {} ADD {named}{what}", quote(table, ctx.dialect)));
    statement.foreign_key = matches!(constraint, NewConstraint::ForeignKey { .. });
    statement
}

pub fn drop_constraint(table: &str, name: &str, ctx: &Context) -> Statement {
    Statement::plain(format!(
        "ALTER TABLE {} DROP CONSTRAINT {}",
        quote(table, ctx.dialect),
        quote(name, ctx.dialect)
    ))
}

/// An index to create, as typed
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NewIndex {
    pub name: String,
    /// Columns, separated by commas; not used when there is an expression
    pub columns: String,
    pub expression: String,
    pub unique: bool,
    pub descending: bool,
}

pub fn create_index(table: &str, index: &NewIndex, ctx: &Context) -> Statement {
    let unique = if index.unique { "UNIQUE " } else { "" };
    let order = if index.descending { "DESCENDING " } else { "" };
    let on = match given(&index.expression) {
        Some(expression) => format!("COMPUTED BY {}", in_parentheses(expression)),
        None => format!("({})", typed_list(&index.columns, ctx)),
    };
    Statement::plain(format!(
        "CREATE {unique}{order}INDEX {} ON {} {on}",
        typed(&index.name, ctx),
        quote(table, ctx.dialect)
    ))
}

/// Turns an index on or off; turning it on rebuilds it
pub fn index_active(name: &str, active: bool, ctx: &Context) -> Statement {
    let state = if active { "ACTIVE" } else { "INACTIVE" };
    Statement::plain(format!("ALTER INDEX {} {state}", quote(name, ctx.dialect)))
}

/// Computes an index's selectivity again
pub fn index_statistics(name: &str, ctx: &Context) -> Statement {
    Statement::plain(format!("SET STATISTICS INDEX {}", quote(name, ctx.dialect)))
}

pub fn trigger_active(name: &str, active: bool, ctx: &Context) -> Statement {
    let state = if active { "ACTIVE" } else { "INACTIVE" };
    Statement::plain(format!("ALTER TRIGGER {} {state}", quote(name, ctx.dialect)))
}

/// What to change of a domain; each `None` leaves that as it is
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DomainChange {
    pub rename: Option<String>,
    pub data_type: Option<String>,
    /// `Some("")` drops the default
    pub default: Option<String>,
    /// `Some("")` drops the check
    pub check: Option<String>,
    /// Whether it has a check now: a domain has one at most, replaced by
    /// dropping it first
    pub has_check: bool,
    /// Firebird 3 and later
    pub not_null: Option<bool>,
}

pub fn alter_domain(name: &str, change: &DomainChange, ctx: &Context) -> Vec<Statement> {
    let domain = quote(name, ctx.dialect);
    let alter = |what: String| Statement::plain(format!("ALTER DOMAIN {domain} {what}"));
    let mut statements = Vec::new();
    if let Some(data_type) = change.data_type.as_deref().and_then(given) {
        statements.push(alter(format!("TYPE {data_type}")));
    }
    if let Some(default) = &change.default {
        statements.push(match given(default) {
            Some(default) => alter(format!("SET {}", led_by("DEFAULT", default))),
            None => alter("DROP DEFAULT".into()),
        });
    }
    if let Some(check) = &change.check {
        if change.has_check {
            statements.push(alter("DROP CONSTRAINT".into()));
        }
        if let Some(check) = given(check) {
            let condition =
                if check.len() > 5 && check[..5].eq_ignore_ascii_case("CHECK") { check.to_string() } else { format!("CHECK {}", in_parentheses(check)) };
            statements.push(alter(format!("ADD CONSTRAINT {condition}")));
        }
    }
    if let Some(not_null) = change.not_null {
        statements.push(alter(if not_null { "SET NOT NULL" } else { "DROP NOT NULL" }.into()));
    }
    if let Some(new) = change.rename.as_deref().and_then(given) {
        statements.push(alter(format!("TO {}", typed(new, ctx))));
    }
    statements
}

/// Sets a generator's value: the next one it gives is the one after
pub fn set_generator(name: &str, value: i64, ctx: &Context) -> Statement {
    // Unlike RESTART WITH, this means the same on every version
    Statement::plain(format!("SET GENERATOR {} TO {value}", quote(name, ctx.dialect)))
}

pub fn alter_exception(name: &str, message: &str, ctx: &Context) -> Statement {
    Statement::plain(format!("ALTER EXCEPTION {} {}", quote(name, ctx.dialect), literal(message)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> Context {
        Context { dialect: 3, charset: "UTF8".into(), major: 4 }
    }

    #[test]
    fn a_typed_name_is_stored_as_sql_would_have_it() {
        assert_eq!(stored(" nome "), "NOME");
        assert_eq!(stored("\"Nome\""), "Nome");
        assert_eq!(stored("\"a \"\"b\"\"\""), "a \"b\"");
        assert_eq!(stored("ação"), "ação");
        for name in ["nome", "\"Nome\"", "com espaço"] {
            assert_eq!(typed(name, &ctx()), quote(&stored(name), 3), "{name}");
        }
    }

    #[test]
    fn a_description_is_set_or_removed() {
        let set = describe(ObjectKind::Table, "PEDIDO", "It's the orders\n", &ctx());
        assert_eq!(set.sql, "COMMENT ON TABLE PEDIDO IS 'It''s the orders'");
        let removed = describe(ObjectKind::Generator, "Gen Id", "  \n", &ctx());
        assert_eq!(removed.sql, "COMMENT ON SEQUENCE \"Gen Id\" IS NULL");
    }

    #[test]
    fn every_kind_drops_and_has_a_template() {
        assert_eq!(drop(ObjectKind::Udf, "STRLEN", &ctx()).sql, "DROP EXTERNAL FUNCTION STRLEN");
        for kind in ObjectKind::ALL {
            let script = template(kind, &ctx());
            assert!(script.ends_with(";\n") || script.ends_with("SET TERM ; ^\n"), "{kind:?}: {script}");
            // What the splitter makes of it is what the server will get
            assert!(fb_sql::split(&script).iter().all(|s| !s.unterminated_literal), "{kind:?}");
        }
        assert!(template(ObjectKind::Procedure, &ctx()).starts_with("SET TERM ^ ;\n\nCREATE OR ALTER PROCEDURE"));
    }

    fn sql(statements: &[Statement]) -> Vec<&str> {
        statements.iter().map(|s| s.sql.as_str()).collect()
    }

    #[test]
    fn typed_names_are_what_sql_would_make_of_them() {
        assert_eq!(typed(" nome ", &ctx()), "NOME");
        assert_eq!(typed("Nome Completo", &ctx()), "\"Nome Completo\"");
        assert_eq!(typed("\"Nome\"", &ctx()), "\"Nome\"");
        // Reserved, so quoted, and in the case the server would keep
        assert_eq!(typed("value", &ctx()), "\"VALUE\"");
        assert_eq!(typed_list("id, \"Nome\" ,, obs", &ctx()), "ID, \"Nome\", OBS");
    }

    #[test]
    fn columns_are_added_changed_and_dropped() {
        let column = NewColumn {
            name: "obs".into(),
            data_type: "varchar(60)".into(),
            not_null: true,
            default: "'x'".into(),
            description: "It's a note".into(),
            ..Default::default()
        };
        assert_eq!(
            sql(&add_column("PEDIDO", &column, &ctx())),
            [
                "ALTER TABLE PEDIDO ADD OBS varchar(60) DEFAULT 'x' NOT NULL",
                "COMMENT ON COLUMN PEDIDO.OBS IS 'It''s a note'"
            ]
        );
        let computed = NewColumn { name: "TOTAL".into(), computed: "QTD * PRECO".into(), ..Default::default() };
        assert_eq!(sql(&add_column("ITEM", &computed, &ctx())), ["ALTER TABLE ITEM ADD TOTAL COMPUTED BY (QTD * PRECO)"]);

        let change = ColumnChange {
            rename: Some("nota".into()),
            data_type: Some("VARCHAR(80)".into()),
            default: Some(String::new()),
            not_null: Some(false),
            position: Some(2),
            description: Some(String::new()),
        };
        assert_eq!(
            sql(&alter_column("PEDIDO", "OBS", &change, &ctx())),
            [
                "ALTER TABLE PEDIDO ALTER COLUMN OBS TYPE VARCHAR(80)",
                "ALTER TABLE PEDIDO ALTER COLUMN OBS DROP DEFAULT",
                "ALTER TABLE PEDIDO ALTER COLUMN OBS DROP NOT NULL",
                "ALTER TABLE PEDIDO ALTER COLUMN OBS POSITION 2",
                "COMMENT ON COLUMN PEDIDO.OBS IS NULL",
                "ALTER TABLE PEDIDO ALTER COLUMN OBS TO NOTA",
            ]
        );
        let default = ColumnChange { default: Some("default 0".into()), ..Default::default() };
        assert_eq!(sql(&alter_column("T", "C", &default, &ctx())), ["ALTER TABLE T ALTER COLUMN C SET default 0"]);
        assert!(alter_column("T", "C", &ColumnChange::default(), &ctx()).is_empty());
        assert_eq!(drop_column("PEDIDO", "Obs", &ctx()).sql, "ALTER TABLE PEDIDO DROP \"Obs\"");
    }

    #[test]
    fn constraints_and_indices_are_added() {
        let key = NewConstraint::ForeignKey {
            columns: "pedido".into(),
            table: "pedido".into(),
            references: "id".into(),
            on_update: "RESTRICT".into(),
            on_delete: "cascade".into(),
        };
        let added = add_constraint("ITEM", "fk_item", &key, &ctx());
        assert_eq!(
            added.sql,
            "ALTER TABLE ITEM ADD CONSTRAINT FK_ITEM FOREIGN KEY (PEDIDO) REFERENCES PEDIDO (ID) ON DELETE CASCADE"
        );
        assert!(added.foreign_key);
        let unnamed = add_constraint("ITEM", " ", &NewConstraint::PrimaryKey { columns: "a,b".into() }, &ctx());
        assert_eq!(unnamed.sql, "ALTER TABLE ITEM ADD PRIMARY KEY (A, B)");
        for condition in ["QTD > 0", "(QTD > 0)", "check (QTD > 0)"] {
            let check = add_constraint("ITEM", "CK", &NewConstraint::Check { condition: condition.into() }, &ctx());
            assert!(check.sql.to_uppercase().ends_with("ADD CONSTRAINT CK CHECK (QTD > 0)"), "{}", check.sql);
        }
        assert_eq!(drop_constraint("ITEM", "CK", &ctx()).sql, "ALTER TABLE ITEM DROP CONSTRAINT CK");

        let index = NewIndex { name: "idx_nome".into(), columns: "nome, id".into(), unique: true, descending: true, ..Default::default() };
        assert_eq!(create_index("T", &index, &ctx()).sql, "CREATE UNIQUE DESCENDING INDEX IDX_NOME ON T (NOME, ID)");
        let computed = NewIndex { name: "IDX_UP".into(), expression: "UPPER(NOME)".into(), ..Default::default() };
        assert_eq!(create_index("T", &computed, &ctx()).sql, "CREATE INDEX IDX_UP ON T COMPUTED BY (UPPER(NOME))");
        assert_eq!(index_active("IDX_UP", false, &ctx()).sql, "ALTER INDEX IDX_UP INACTIVE");
        assert_eq!(trigger_active("T_BI", true, &ctx()).sql, "ALTER TRIGGER T_BI ACTIVE");
    }

    #[test]
    fn small_objects_are_changed() {
        let change = DomainChange {
            data_type: Some("VARCHAR(10)".into()),
            default: Some("'N'".into()),
            check: Some("VALUE IN ('S','N')".into()),
            has_check: true,
            rename: Some("d_sn".into()),
            ..Default::default()
        };
        assert_eq!(
            sql(&alter_domain("D_FLAG", &change, &ctx())),
            [
                "ALTER DOMAIN D_FLAG TYPE VARCHAR(10)",
                "ALTER DOMAIN D_FLAG SET DEFAULT 'N'",
                "ALTER DOMAIN D_FLAG DROP CONSTRAINT",
                "ALTER DOMAIN D_FLAG ADD CONSTRAINT CHECK (VALUE IN ('S','N'))",
                "ALTER DOMAIN D_FLAG TO D_SN",
            ]
        );
        assert_eq!(set_generator("GEN_ID", 41, &ctx()).sql, "SET GENERATOR GEN_ID TO 41");
        assert_eq!(alter_exception("E_X", "can't", &ctx()).sql, "ALTER EXCEPTION E_X 'can''t'");
    }
}
