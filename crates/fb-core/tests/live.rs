//! Against a live server, named by the `FB_*` environment variables (see
//! `ConnectParams::from_env`). Skipped without them. Read-only: these also
//! run against real databases.

use fb_core::{ConnectParams, Connection, Decimal, Error, Value};

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
            vec![
                Value::Decimal("-12345.6789".parse().unwrap()),
                "ação".into(),
                Value::Integer(42),
            ],
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
    let Some(mut params) = ConnectParams::from_env() else { return };
    params.password.push_str("-wrong");
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
