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

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> Context {
        Context { dialect: 3, charset: "UTF8".into(), major: 4 }
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
}
