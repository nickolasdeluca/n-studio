//! Against a live server, named by the `FB_*` environment variables (see
//! `ConnectParams::from_env`). Skipped without them. Read-only: these also
//! run against real databases.

use fb_core::{
    ColumnKind, ConnectParams, Connection, Decimal, Error, ExecuteOptions, Isolation, LockWait, StatementType, TxEnd,
    TxOptions, Value,
};

fn connect() -> Option<Connection> {
    let Some(params) = ConnectParams::from_env() else {
        eprintln!("FB_DB not set: skipping live test");
        return None;
    };
    Some(Connection::open(&params).expect("connect"))
}

#[test]
fn reports_the_server() {
    let Some(conn) = connect() else { return };
    let info = conn.info();
    assert!(info.major >= 2, "{info:?}");
    assert!(info.engine_version.starts_with(&format!("{}.{}", info.major, info.minor)), "{info:?}");
    assert!(info.ods_major >= 11, "{info:?}");
    assert!(info.page_size >= 1024, "{info:?}");
    assert_eq!(info.dialect, 3, "{info:?}");
    assert!(!info.charset.is_empty(), "{info:?}");
}

#[test]
fn reads_and_binds_values() {
    let Some(conn) = connect() else { return };
    let result = conn
        .query(
            "select cast(? as numeric(18,4)), cast(? as varchar(20)), cast(? as integer), \
                    cast(null as integer) from rdb$database",
            vec![Value::Decimal("-12345.6789".parse().unwrap()), "ação".into(), Value::Integer(42)],
        )
        .unwrap();
    assert_eq!(result.columns.len(), 4);
    assert_eq!(
        result.rows,
        vec![vec![
            Value::Decimal(Decimal::new(-123456789, 4)),
            Value::Text("ação".into()),
            Value::Integer(42),
            Value::Null,
        ]]
    );
}

#[test]
fn server_errors_keep_their_sqlcode() {
    let Some(conn) = connect() else { return };
    match conn.query("select * from NO_SUCH_TABLE_HERE", vec![]) {
        Err(Error::Server { sqlcode, message }) => {
            assert_eq!(sqlcode, -204, "{message}");
            assert!(message.contains("NO_SUCH_TABLE_HERE"), "{message}");
        }
        other => panic!("expected a server error, got {other:?}"),
    }
    // The connection survives the error.
    assert_eq!(conn.query("select 1 from rdb$database", vec![]).unwrap().rows.len(), 1);
}

#[test]
fn a_wrong_password_is_refused() {
    let Some(mut params) = ConnectParams::from_env() else {
        return;
    };
    // At the front: Legacy_Auth (2.5, and 3.0+ kept compatible with it)
    // compares only the first 8 characters.
    params.password.insert_str(0, "wrong-");
    match Connection::open(&params) {
        Err(Error::Server { .. }) => {}
        Err(other) => panic!("expected the server to refuse, got {other:?}"),
        Ok(_) => panic!("connected with a wrong password"),
    }
}

#[test]
fn an_unreachable_server_is_a_network_error() {
    let params = ConnectParams { host: "127.0.0.1".into(), port: 1, database: "x".into(), ..Default::default() };
    assert!(matches!(Connection::open(&params), Err(Error::Network { .. })));
}

fn read_only() -> TxOptions {
    TxOptions { isolation: Isolation::Snapshot, read_only: true, ..Default::default() }
}

#[test]
fn an_editor_select_fetches_in_pages() {
    let Some(conn) = connect() else { return };
    let total = conn.query("select count(*) from rdb$relations", vec![]).unwrap().rows[0][0].as_i64().unwrap();

    let tx = conn.begin(read_only()).unwrap();
    let executed = conn
        .execute(tx, "select rdb$relation_id, rdb$relation_name from rdb$relations", ExecuteOptions::default())
        .unwrap();
    assert_eq!(executed.statement_type, StatementType::Select);
    assert_eq!(executed.columns.len(), 2);
    assert_eq!(executed.columns[0].kind, ColumnKind::Integer);
    assert_eq!(executed.columns[0].type_name, "SMALLINT");
    assert_eq!(executed.columns[1].relation, "RDB$RELATIONS");
    assert!(executed.plan.as_deref().unwrap_or_default().contains("RDB$RELATIONS"), "{:?}", executed.plan);
    assert_eq!(executed.explained_plan.is_some(), conn.info().major >= 3);

    let cursor = executed.cursor.expect("a select leaves a cursor");
    let mut rows = 0;
    let performance = loop {
        let page = conn.fetch(cursor, 50).unwrap();
        assert!(page.rows.len() <= 50);
        rows += page.rows.len() as i64;
        if page.eof {
            break page.performance.expect("asked for performance");
        }
        assert!(page.performance.is_none());
    };
    assert_eq!(rows, total);
    let relations = performance.tables.iter().find(|t| t.table == "RDB$RELATIONS").expect("{performance:?}");
    assert!(relations.non_indexed_reads >= total, "{performance:?}");
    assert!(performance.fetches > 0, "{performance:?}");
    // Done with: closing it again is fine, fetching from it is not
    conn.close_cursor(cursor).unwrap();
    assert!(conn.fetch(cursor, 1).is_err());

    // COMMIT typed in the editor ends the editor's transaction
    let committed = conn.execute(tx, "commit", ExecuteOptions::default()).unwrap();
    assert_eq!(committed.ended, Some(TxEnd::Commit));
    assert!(conn.execute(tx, "select 1 from rdb$database", ExecuteOptions::default()).is_err());
}

#[test]
fn ending_a_transaction_closes_its_cursors() {
    let Some(conn) = connect() else { return };
    let tx = conn.begin(read_only()).unwrap();
    let options = ExecuteOptions { plan: false, performance: false };
    let cursor = conn.execute(tx, "select * from rdb$relations", options).unwrap().cursor.unwrap();
    assert_eq!(conn.fetch(cursor, 1).unwrap().rows.len(), 1);

    // Retaining keeps both
    conn.end(tx, TxEnd::RollbackRetaining).unwrap();
    assert_eq!(conn.fetch(cursor, 1).unwrap().rows.len(), 1);

    conn.end(tx, TxEnd::Rollback).unwrap();
    assert!(conn.fetch(cursor, 1).is_err());
    assert!(conn.end(tx, TxEnd::Rollback).is_err());
}

#[test]
fn the_editor_refuses_what_it_cannot_run_and_carries_on() {
    let Some(conn) = connect() else { return };
    let tx = conn.begin(read_only()).unwrap();
    let options = ExecuteOptions::default();
    let refused = |sql: &str| match conn.execute(tx, sql, options) {
        Err(Error::Client { message }) => message,
        other => panic!("{sql}: expected a refusal, got {other:?}"),
    };
    let m = refused("select * from rdb$relations where rdb$relation_id = ?");
    assert!(m.contains("parameters"), "{m}");
    assert!(refused("set transaction read committed").contains("SET TRANSACTION"));
    assert!(matches!(conn.execute(tx, "select * from NO_SUCH_TABLE_HERE", options), Err(Error::Server { .. })));

    // Still usable after all that
    let executed = conn.execute(tx, "execute block returns (x integer) as begin x = 7; suspend; end", options).unwrap();
    let page = conn.fetch(executed.cursor.unwrap(), 10).unwrap();
    assert_eq!(page.rows, vec![vec![Value::Integer(7)]]);
    assert!(page.eof);
    conn.end(tx, TxEnd::Rollback).unwrap();
}

/// Writes, so only against the fixture database, and always rolled back
#[test]
fn fixture_writes_report_records_and_table_counters() {
    if std::env::var("FB_FIXTURE").as_deref() != Ok("1") {
        return;
    }
    let Some(conn) = connect() else { return };
    let tx = conn.begin(TxOptions { lock_wait: LockWait::NoWait, ..Default::default() }).unwrap();
    let options = ExecuteOptions::default();

    let inserted = conn.execute(tx, "insert into tipos (id, c_integer) values (999999, 1)", options).unwrap();
    assert_eq!(inserted.statement_type, StatementType::Insert);
    assert_eq!(inserted.records.inserted, 1);
    let performance = inserted.performance.expect("no cursor: final already");
    let tipos = performance.tables.iter().find(|t| t.table == "TIPOS").expect("{performance:?}");
    assert_eq!(tipos.inserts, 1);

    let updated = conn.execute(tx, "update tipos set c_integer = 2 where id = 999999", options).unwrap();
    assert!(updated.records.updated >= 1);

    let procedure = conn.execute(tx, "execute procedure p_fixture(3)", options).unwrap();
    assert_eq!(procedure.statement_type, StatementType::ExecProcedure);
    assert_eq!(procedure.rows, vec![vec![Value::Integer(3), Value::Text("proc row 3".into())]]);

    let column = |sql: &str| conn.execute(tx, sql, options).unwrap().columns.remove(0).type_name;
    assert_eq!(column("select c_numeric_18_4 from tipos"), "NUMERIC(18,4)");
    assert_eq!(column("select c_vc_utf8 from tipos"), "VARCHAR(100)");
    assert_eq!(column("select c_char from tipos"), "CHAR(10)");

    let rolled_back = conn.execute(tx, "rollback", options).unwrap();
    assert_eq!(rolled_back.ended, Some(TxEnd::Rollback));
}
