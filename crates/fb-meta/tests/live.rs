//! Against a live server, named by the `FB_*` environment variables (see
//! `ConnectParams::from_env`). Skipped without them. With `FB_FIXTURE=1` the
//! database is taken to be the one docker/seed builds, and checked object by
//! object.

use fb_core::{ConnectParams, Connection};
use fb_meta::{
    counts, dependencies, extract, objects, table, tables, ConstraintKind, DbObject, Dependency, Generation, ObjectKind,
};

fn connect() -> Option<Connection> {
    let Some(params) = ConnectParams::from_env() else {
        eprintln!("FB_DB not set: skipping live test");
        return None;
    };
    Some(Connection::open(&params).expect("connect"))
}

fn names(objects: &[DbObject]) -> Vec<&str> {
    objects.iter().map(|o| o.name.as_str()).collect()
}

#[test]
fn counts_agree_with_the_lists() {
    let Some(conn) = connect() else { return };
    let counts = counts(&conn).unwrap();
    let kinds: Vec<_> = Generation::of(conn.info()).kinds().collect();
    assert_eq!(counts.iter().map(|(k, _)| *k).collect::<Vec<_>>(), kinds);

    for (kind, count) in counts {
        let list = objects(&conn, kind).unwrap();
        assert_eq!(list.len() as u64, count, "{kind:?}");
        let mut sorted = names(&list);
        sorted.sort();
        assert_eq!(names(&list), sorted, "{kind:?} comes back by name");
        for o in &list {
            assert!(!o.name.is_empty() && o.name.trim() == o.name, "{kind:?} {o:?}");
            assert!(
                !o.name.starts_with("RDB$") && !o.name.starts_with("MON$") || kind == ObjectKind::Index,
                "{kind:?}: system object {o:?}"
            );
        }
    }
}

#[test]
fn the_fixture_lists_what_it_seeded() {
    if std::env::var("FB_FIXTURE").as_deref() != Ok("1") {
        return;
    }
    let Some(conn) = connect() else { return };
    let fb3 = Generation::of(conn.info()) == Generation::Fb3;
    let major = conn.info().major;
    let list = |kind| objects(&conn, kind).unwrap();

    assert_eq!(names(&list(ObjectKind::Domain)), ["D_FLAG", "D_ID", "D_MONEY"]);
    let mut tables = vec!["BULK_ROWS", "PEDIDO", "PEDIDO_ITEM", "TIPOS"];
    if major >= 3 {
        tables.push("TIPOS_FB3");
    }
    if major >= 4 {
        tables.push("TIPOS_FB4");
    }
    assert_eq!(names(&list(ObjectKind::Table)), tables);
    assert_eq!(names(&list(ObjectKind::View)), ["V_EVEN", "V_PEDIDO_TOTAL", "V_TIPOS"]);
    assert_eq!(names(&list(ObjectKind::Procedure)), ["P_EVEN", "P_FIXTURE", "P_ODD", "P_PEDIDO_TOTAL", "P_SEED_BULK"]);
    // The identity column's sequence is the engine's, not the user's
    assert_eq!(names(&list(ObjectKind::Generator)), ["GEN_TIPOS_ID"]);
    assert_eq!(names(&list(ObjectKind::Exception)), ["E_FIXTURE"]);
    assert_eq!(
        list(ObjectKind::Trigger),
        [DbObject { name: "TIPOS_BI".into(), parent: Some("TIPOS".into()), description: None }]
    );
    let indices = list(ObjectKind::Index);
    let idx = indices.iter().find(|o| o.name == "IDX_BULK_NOME").expect("IDX_BULK_NOME");
    assert_eq!(idx.parent.as_deref(), Some("BULK_ROWS"));
    if fb3 {
        assert_eq!(names(&list(ObjectKind::Package)), ["PKG_FIXTURE"]);
        // PKG_FIXTURE.DOBRO belongs under its package
        assert_eq!(names(&list(ObjectKind::Function)), Vec::<&str>::new());
    }
}

fn fixture() -> Option<Connection> {
    if std::env::var("FB_FIXTURE").as_deref() != Ok("1") {
        return None;
    }
    connect()
}

/// A few of a list, from its start, middle and end
fn sample<T: Clone>(list: &[T], n: usize) -> Vec<T> {
    if list.len() <= n {
        return list.to_vec();
    }
    (0..n).map(|i| list[i * (list.len() - 1) / (n - 1)].clone()).collect()
}

#[test]
fn every_kind_scripts_and_resolves_its_dependencies() {
    let Some(conn) = connect() else { return };
    for kind in Generation::of(conn.info()).kinds() {
        for object in sample(&objects(&conn, kind).unwrap(), 4) {
            let ddl = extract::script(&conn, kind, &object.name).unwrap_or_else(|e| panic!("{kind:?}: {e}"));
            assert!(ddl.ends_with(";\n") || ddl.ends_with("SET TERM ; ^\n"), "{kind:?} DDL ends oddly");
            // `TYPE <code>` is what an unknown type renders as; a blob's
            // `SUB_TYPE 0` is not one
            let types = ddl.replace("SUB_TYPE", "");
            assert!(
                !types.contains("TYPE 0") && !types.contains("TYPE -"),
                "{kind:?}: a type the catalog reader does not know"
            );
            dependencies(&conn, kind, &object.name).unwrap_or_else(|e| panic!("{kind:?} dependencies: {e}"));
        }
    }
}

#[test]
fn all_tables_at_once_read_as_each_alone() {
    let Some(conn) = connect() else { return };
    let listed = objects(&conn, ObjectKind::Table).unwrap();
    let all = tables(&conn, None).unwrap();
    assert_eq!(all.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), names(&listed));
    for one in sample(&all, 5) {
        assert!(!one.columns.is_empty(), "a table without columns");
        assert_eq!(table(&conn, &one.name).unwrap(), one);
        for c in &one.constraints {
            assert_eq!(c.columns.is_empty(), c.kind == ConstraintKind::Check, "only a check has no key columns");
            if let Some(r) = &c.references {
                assert_eq!(r.columns.len(), c.columns.len(), "a foreign key pairs its columns");
            }
        }
    }
}

#[test]
fn the_fixture_scripts_as_it_was_declared() {
    let Some(conn) = fixture() else { return };
    let ddl = |kind, name| extract::script(&conn, kind, name).unwrap();

    assert_eq!(
        ddl(ObjectKind::Table, "PEDIDO"),
        "\
CREATE TABLE PEDIDO (
    ID       D_ID,
    CLIENTE  VARCHAR(60) NOT NULL,
    EMISSAO  DATE DEFAULT CURRENT_DATE NOT NULL
);

ALTER TABLE PEDIDO ADD CONSTRAINT PK_PEDIDO PRIMARY KEY (ID);
ALTER TABLE PEDIDO ADD CONSTRAINT UQ_PEDIDO UNIQUE (CLIENTE, EMISSAO);

CREATE INDEX IDX_PEDIDO_CLIENTE ON PEDIDO COMPUTED BY (UPPER(CLIENTE));

COMMENT ON TABLE PEDIDO IS 'Orders';
COMMENT ON COLUMN PEDIDO.CLIENTE IS 'Who it''s for';
"
    );
    assert_eq!(
        ddl(ObjectKind::Table, "PEDIDO_ITEM"),
        "\
CREATE TABLE PEDIDO_ITEM (
    PEDIDO  D_ID,
    ITEM    SMALLINT NOT NULL,
    QTD     NUMERIC(9,3) DEFAULT 1 NOT NULL,
    PRECO   D_MONEY,
    TOTAL   COMPUTED BY (QTD * PRECO),
    PAGO    D_FLAG
);

ALTER TABLE PEDIDO_ITEM ADD CONSTRAINT PK_PEDIDO_ITEM PRIMARY KEY (PEDIDO, ITEM);
ALTER TABLE PEDIDO_ITEM ADD CONSTRAINT FK_ITEM_PEDIDO FOREIGN KEY (PEDIDO) REFERENCES PEDIDO (ID) ON DELETE CASCADE;
ALTER TABLE PEDIDO_ITEM ADD CONSTRAINT CK_ITEM_QTD CHECK (QTD > 0);

CREATE DESCENDING INDEX IDX_ITEM_PRECO ON PEDIDO_ITEM (PRECO);
"
    );
    // Firebird 2 keeps the query's indentation, later versions drop it
    let query = if conn.info().major >= 3 { "SELECT" } else { "  SELECT" };
    assert_eq!(
        ddl(ObjectKind::View, "V_PEDIDO_TOTAL"),
        format!(
            "\
CREATE OR ALTER VIEW V_PEDIDO_TOTAL (
    PEDIDO,
    TOTAL)
AS
{query} I.PEDIDO, SUM(I.TOTAL) FROM PEDIDO_ITEM I GROUP BY I.PEDIDO;
"
        )
    );
    assert_eq!(
        ddl(ObjectKind::Procedure, "P_PEDIDO_TOTAL"),
        "\
SET TERM ^ ;

CREATE OR ALTER PROCEDURE P_PEDIDO_TOTAL (
    P_PEDIDO TYPE OF D_ID,
    P_MINIMO NUMERIC(18,4) = 0)
RETURNS (
    R_TOTAL TYPE OF COLUMN PEDIDO_ITEM.PRECO)
AS
BEGIN
  SELECT V.TOTAL FROM V_PEDIDO_TOTAL V WHERE V.PEDIDO = :P_PEDIDO INTO :R_TOTAL;
  IF (R_TOTAL < P_MINIMO) THEN
    EXCEPTION E_FIXTURE;
  SUSPEND;
END^

SET TERM ; ^
"
    );
    assert_eq!(
        ddl(ObjectKind::Trigger, "TIPOS_BI"),
        "\
SET TERM ^ ;

CREATE OR ALTER TRIGGER TIPOS_BI FOR TIPOS
ACTIVE BEFORE INSERT POSITION 0
AS
BEGIN
  IF (NEW.ID IS NULL) THEN
    NEW.ID = GEN_ID(GEN_TIPOS_ID, 1);
END^

SET TERM ; ^
"
    );
    assert_eq!(
        ddl(ObjectKind::Domain, "D_FLAG"),
        "CREATE DOMAIN D_FLAG AS CHAR(1)\n    DEFAULT 'N'\n    CHECK (VALUE IN ('S','N'));\n"
    );
    assert_eq!(ddl(ObjectKind::Domain, "D_ID"), "CREATE DOMAIN D_ID AS INTEGER\n    NOT NULL;\n");
    assert_eq!(ddl(ObjectKind::Exception, "E_FIXTURE"), "CREATE EXCEPTION E_FIXTURE 'fixture exception';\n");
    assert_eq!(ddl(ObjectKind::Index, "IDX_BULK_NOME"), "CREATE INDEX IDX_BULK_NOME ON BULK_ROWS (NOME);\n");
    assert!(ddl(ObjectKind::Generator, "GEN_TIPOS_ID").starts_with("CREATE SEQUENCE GEN_TIPOS_ID;\n"));

    // A table brings its triggers, and says which of its columns are not in
    // the database's own character set
    let tipos = ddl(ObjectKind::Table, "TIPOS");
    assert!(tipos.contains("    C_NUMERIC_18_4  NUMERIC(18,4),\n"), "{tipos}");
    assert!(tipos.contains("    C_BLOB_TEXT     BLOB SUB_TYPE TEXT"), "{tipos}");
    assert!(tipos.contains("    C_BLOB_BIN      BLOB SUB_TYPE 0,\n"), "{tipos}");
    assert!(tipos.contains("CREATE OR ALTER TRIGGER TIPOS_BI FOR TIPOS\n"), "{tipos}");
    for charset in ["WIN1252", "UTF8", "NONE"] {
        let declared = tipos.contains(&format!("VARCHAR(100) CHARACTER SET {charset}"));
        assert_eq!(declared, conn.info().charset != charset, "{charset} in {tipos}");
    }

    if conn.info().major >= 3 {
        let fb3 = ddl(ObjectKind::Table, "TIPOS_FB3");
        assert!(fb3.contains("    ID         INTEGER GENERATED BY DEFAULT AS IDENTITY"), "{fb3}");
        assert!(fb3.contains("    C_BOOLEAN  BOOLEAN\n"), "{fb3}");
        let package = ddl(ObjectKind::Package, "PKG_FIXTURE");
        assert!(package.contains("CREATE OR ALTER PACKAGE PKG_FIXTURE\nAS\nBEGIN\n"), "{package}");
        assert!(package.contains("RECREATE PACKAGE BODY PKG_FIXTURE\nAS\nBEGIN\n"), "{package}");
    }
    if conn.info().major >= 4 {
        let fb4 = ddl(ObjectKind::Table, "TIPOS_FB4");
        for declared in
            ["DECFLOAT(16)", "DECFLOAT(34)", "NUMERIC(38,10)", "TIMESTAMP WITH TIME ZONE", "TIME WITH TIME ZONE"]
        {
            assert!(fb4.contains(declared), "{declared} in {fb4}");
        }
    }
}

#[test]
fn the_fixture_knows_what_uses_what() {
    let Some(conn) = fixture() else { return };
    let has = |list: &[Dependency], kind, what: &str, name: &str| {
        list.iter().any(|d| d.kind == Some(kind) && d.what == what && d.name == name)
    };

    let item = dependencies(&conn, ObjectKind::Table, "PEDIDO_ITEM").unwrap();
    assert!(has(&item.uses, ObjectKind::Table, "Foreign key", "PEDIDO"), "{item:?}");
    for domain in ["D_ID", "D_MONEY", "D_FLAG"] {
        assert!(has(&item.uses, ObjectKind::Domain, "Domain", domain), "{domain}: {item:?}");
    }
    assert!(!item.uses.iter().any(|d| d.name == "PEDIDO_ITEM"), "its computed column is no dependency: {item:?}");
    assert!(has(&item.used_by, ObjectKind::View, "View", "V_PEDIDO_TOTAL"), "{item:?}");

    let order = dependencies(&conn, ObjectKind::Table, "PEDIDO").unwrap();
    let key = order.used_by.iter().find(|d| d.what == "Foreign key").expect("the foreign key to PEDIDO");
    assert_eq!((key.name.as_str(), key.detail.as_deref()), ("PEDIDO_ITEM", Some("FK_ITEM_PEDIDO")));

    let total = dependencies(&conn, ObjectKind::Procedure, "P_PEDIDO_TOTAL").unwrap();
    assert!(has(&total.uses, ObjectKind::View, "View", "V_PEDIDO_TOTAL"), "{total:?}");
    assert!(has(&total.uses, ObjectKind::Exception, "Exception", "E_FIXTURE"), "{total:?}");

    let exception = dependencies(&conn, ObjectKind::Exception, "E_FIXTURE").unwrap();
    assert!(has(&exception.used_by, ObjectKind::Procedure, "Procedure", "P_PEDIDO_TOTAL"), "{exception:?}");

    let generator = dependencies(&conn, ObjectKind::Generator, "GEN_TIPOS_ID").unwrap();
    let trigger = generator.used_by.iter().find(|d| d.name == "TIPOS_BI").expect("the trigger that uses it");
    assert_eq!((trigger.kind, trigger.detail.as_deref()), (Some(ObjectKind::Trigger), Some("TIPOS")));

    let view = dependencies(&conn, ObjectKind::View, "V_PEDIDO_TOTAL").unwrap();
    assert!(has(&view.uses, ObjectKind::Table, "Table", "PEDIDO_ITEM"), "{view:?}");
    assert!(has(&view.used_by, ObjectKind::Procedure, "Procedure", "P_PEDIDO_TOTAL"), "{view:?}");
}

#[test]
fn a_selection_scripts_in_an_order_that_runs() {
    let Some(conn) = fixture() else { return };
    let pick = |kind, name: &str| (kind, name.to_string());
    let script = fb_meta::ddl::script(
        &extract::selection(
            &conn,
            &[
                pick(ObjectKind::Trigger, "TIPOS_BI"),
                pick(ObjectKind::View, "V_PEDIDO_TOTAL"),
                pick(ObjectKind::Table, "PEDIDO_ITEM"),
                pick(ObjectKind::Table, "PEDIDO"),
                pick(ObjectKind::Table, "TIPOS"),
                pick(ObjectKind::Index, "IDX_ITEM_PRECO"),
                pick(ObjectKind::Domain, "D_ID"),
            ],
        )
        .unwrap(),
    );
    let at = |text: &str| script.find(text).unwrap_or_else(|| panic!("{text} in {script}"));
    assert!(at("CREATE DOMAIN D_ID") < at("CREATE TABLE PEDIDO_ITEM ("), "{script}");
    // The referenced table was picked after the one referencing it
    assert!(at("CREATE TABLE PEDIDO_ITEM (") < at("CREATE TABLE PEDIDO ("), "{script}");
    assert!(at("CREATE TABLE PEDIDO (") < at("ADD CONSTRAINT FK_ITEM_PEDIDO FOREIGN KEY"), "{script}");
    assert!(at("CREATE TABLE TIPOS (") < at("ADD CONSTRAINT FK_ITEM_PEDIDO FOREIGN KEY"), "{script}");
    assert!(at("ADD CONSTRAINT FK_ITEM_PEDIDO FOREIGN KEY") < at("CREATE OR ALTER VIEW V_PEDIDO_TOTAL"), "{script}");
    assert!(at("CREATE OR ALTER VIEW V_PEDIDO_TOTAL") < at("CREATE OR ALTER TRIGGER TIPOS_BI"), "{script}");
    // What came with its table is there once
    for once in ["CREATE OR ALTER TRIGGER TIPOS_BI", "CREATE DESCENDING INDEX IDX_ITEM_PRECO"] {
        assert_eq!(script.matches(once).count(), 1, "{once} in {script}");
    }
    assert!(script.ends_with("SET TERM ; ^\n"), "{script}");
}

#[test]
fn the_fixture_grants_what_it_was_told_to() {
    let Some(conn) = fixture() else { return };
    let ddl = |kind, name| extract::script(&conn, kind, name).unwrap();

    // What the owner holds for having created the table is not a grant
    let table = ddl(ObjectKind::Table, "BULK_ROWS");
    assert!(
        table.ends_with(
            "\n\
GRANT SELECT ON BULK_ROWS TO PROCEDURE P_FIXTURE;
GRANT SELECT, UPDATE (NOME, VALOR) ON BULK_ROWS TO ROLE R_FIXTURE;
GRANT ALL ON BULK_ROWS TO U_FIXTURE WITH GRANT OPTION;
"
        ),
        "{table}"
    );
    assert_eq!(table.lines().filter(|line| line.starts_with("GRANT ")).count(), 3, "{table}");
    let view = ddl(ObjectKind::View, "V_TIPOS");
    assert!(view.ends_with("\nGRANT SELECT ON V_TIPOS TO PUBLIC;\n"), "{view}");
    let procedure = ddl(ObjectKind::Procedure, "P_SEED_BULK");
    assert!(procedure.ends_with("SET TERM ; ^\n\nGRANT EXECUTE ON PROCEDURE P_SEED_BULK TO U_FIXTURE;\n"), "{procedure}");
    assert_eq!(ddl(ObjectKind::Role, "R_FIXTURE"), "CREATE ROLE R_FIXTURE;\n\nGRANT R_FIXTURE TO U_FIXTURE WITH ADMIN OPTION;\n");
    // An object nobody was granted anything on says nothing of grants
    assert!(!ddl(ObjectKind::Table, "PEDIDO").contains("GRANT"));

    let held = fb_meta::grant::held_by_role(&conn, "R_FIXTURE").unwrap();
    let held: Vec<_> = held.iter().map(|g| (g.object.as_str(), g.privilege, g.column.as_deref())).collect();
    assert_eq!(held, [("BULK_ROWS", 'S', None), ("BULK_ROWS", 'U', Some("NOME")), ("BULK_ROWS", 'U', Some("VALOR"))]);

    // In a selection the grants wait for whatever they are granted to
    let picked = [(ObjectKind::Table, "BULK_ROWS".to_string()), (ObjectKind::Procedure, "P_FIXTURE".to_string())];
    let script = fb_meta::ddl::script(&extract::selection(&conn, &picked).unwrap());
    let at = |text: &str| script.find(text).unwrap_or_else(|| panic!("{text} in {script}"));
    assert!(at("PROCEDURE P_FIXTURE") < at("GRANT SELECT ON BULK_ROWS TO PROCEDURE P_FIXTURE"), "{script}");
    assert_eq!(script.matches("GRANT ALL ON BULK_ROWS").count(), 1, "{script}");
}

/// Where `CREATE OR ALTER <what> <name>` is in a script: every place, in
/// order. A longer name that starts the same is not it.
fn creations(script: &str, what: &str, name: &str) -> Vec<usize> {
    let wanted = format!("CREATE OR ALTER {what} {name}");
    script
        .match_indices(&wanted)
        .filter(|(at, _)| {
            let after = script[at + wanted.len()..].chars().next();
            !after.is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '$')
        })
        .map(|(at, _)| at)
        .collect()
}

#[test]
fn routines_that_call_each_other_are_declared_first() {
    let Some(conn) = fixture() else { return };
    let pick = |kind, name: &str| (kind, name.to_string());
    let picked = [
        pick(ObjectKind::View, "V_EVEN"),
        pick(ObjectKind::Procedure, "P_ODD"),
        pick(ObjectKind::Procedure, "P_EVEN"),
        pick(ObjectKind::Procedure, "P_FIXTURE"),
    ];
    let script = fb_meta::ddl::script(&extract::selection(&conn, &picked).unwrap());
    let even = creations(&script, "PROCEDURE", "P_EVEN");
    let odd = creations(&script, "PROCEDURE", "P_ODD");
    let view = creations(&script, "VIEW", "V_EVEN");
    // Each of the two is there empty, then whole; the view after both
    assert_eq!((even.len(), odd.len(), view.len()), (2, 2, 1), "{script}");
    assert!(even[0] < odd[1] && odd[0] < even[1], "{script}");
    assert!(even[0] < view[0], "{script}");
    assert!(script.contains("CREATE OR ALTER PROCEDURE P_EVEN (\n    N INTEGER\n)\nRETURNS (\n    R SMALLINT\n)\nAS\nBEGIN\n    SUSPEND;\nEND^"), "{script}");
    // One that calls nobody back is there once
    assert_eq!(creations(&script, "PROCEDURE", "P_FIXTURE").len(), 1, "{script}");
}

/// On any database: whatever a routine or a view of a selection uses, of
/// the selection, is created before it is defined
#[test]
fn a_selection_creates_nothing_before_what_it_uses() {
    let Some(conn) = connect() else { return };
    let dialect = conn.info().dialect;
    let mut picked = Vec::new();
    for kind in [ObjectKind::Procedure, ObjectKind::View] {
        let list = objects(&conn, kind).unwrap();
        picked.extend(sample(&list, 40).into_iter().map(|o| (kind, o.name)));
    }
    let script = fb_meta::ddl::script(&extract::selection(&conn, &picked).unwrap());
    let what = |kind| if kind == ObjectKind::View { "VIEW" } else { "PROCEDURE" };
    let places = |kind, name: &str| creations(&script, what(kind), &fb_meta::quote(name, dialect));
    for (kind, name) in &picked {
        let defined = *places(*kind, name).last().unwrap_or_else(|| panic!("{kind:?} {name} is not in the script"));
        for used in dependencies(&conn, *kind, name).unwrap().uses {
            let Some(used_kind) = used.kind.filter(|k| matches!(k, ObjectKind::Procedure | ObjectKind::View)) else { continue };
            if used.detail.is_some() && used_kind == ObjectKind::Procedure || (used_kind, &used.name) == (*kind, name) {
                continue;
            }
            if !picked.contains(&(used_kind, used.name.clone())) {
                continue;
            }
            let created = places(used_kind, &used.name)[0];
            assert!(created < defined, "a {kind:?} is defined before the {used_kind:?} it uses is created");
        }
    }
}

