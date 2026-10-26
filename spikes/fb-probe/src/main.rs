//! M0 spike: validates the pure-Rust Firebird driver against a live server.
//!
//! Answers the four questions M0 exists to answer:
//!   1. does the pure-Rust backend connect to 2.5 / 3.0 / 4.0 / 5.0?
//!   2. do WIN1252 and UTF8 text round-trip intact?
//!   3. does every SQL type we care about map to something usable?
//!   4. how fast does a large result set stream, and at what batch size?
//!
//! Usage:
//!   FB_HOST=localhost FB_PORT=3050 FB_DB=/path/fixture.fdb \
//!   FB_USER=SYSDBA FB_PASS=masterkey cargo run -p fb-probe
//!
//!   cargo run -p fb-probe -- --matrix        # every port in docker/compose.yml
//!   cargo run -p fb-probe -- --bulk 1000000  # streaming throughput
//!   cargo run -p fb-probe -- --perf "select ..."  # plan + performance block

use anyhow::{Context, Result};
use rsfbclient::{prelude::*, Charset, Dialect, Row, SqlType};
use rsfbclient_core::Decimal;
use std::env;
use std::time::Instant;

mod perf;

/// One server in the test matrix.
struct Target {
    label: &'static str,
    port: u16,
}

const MATRIX: &[Target] = &[
    Target { label: "fb2.5", port: 3050 },
    Target { label: "fb3.0", port: 3053 },
    Target { label: "fb4.0", port: 3054 },
    Target { label: "fb5.0", port: 3055 },
];

/// Text we wrote into the fixture; anything else coming back means the
/// charset layer mangled it.
const EXPECTED_ACCENTED: &str = "ação preço ãõçéêà";

fn main() -> Result<()> {
    let args: Vec<String> = env::args().skip(1).collect();
    let bulk_rows: usize = arg_value(&args, "--bulk").unwrap_or(0);

    if let Some(sql) = arg_value::<String>(&args, "--perf") {
        return perf::perf(&sql);
    }

    if args.iter().any(|a| a == "--survey") {
        let port = env_or("FB_PORT", "3050").parse().context("FB_PORT")?;
        return survey(port);
    }

    if args.iter().any(|a| a == "--matrix") {
        let mut failures = 0;
        for target in MATRIX {
            println!("\n════════ {} (port {}) ════════", target.label, target.port);
            if let Err(e) = probe(target.port, bulk_rows) {
                failures += 1;
                println!("  FAILED: {e:#}");
            }
        }
        println!("\n{} of {} targets failed", failures, MATRIX.len());
        if failures > 0 {
            std::process::exit(1);
        }
    } else {
        let port = env_or("FB_PORT", "3050").parse().context("FB_PORT")?;
        probe(port, bulk_rows)?;
    }
    Ok(())
}

fn probe(port: u16, bulk_rows: usize) -> Result<()> {
    let host = env_or("FB_HOST", "localhost");
    let db = env_or("FB_DB", "fixture.fdb");
    let user = env_or("FB_USER", "SYSDBA");
    let pass = env_or("FB_PASS", "masterkey");
    let charset = charset_by_name(&env_or("FB_CHARSET", "UTF8"))?;

    let t0 = Instant::now();
    let mut conn = rsfbclient::builder_pure_rust()
        .host(&host)
        .port(port)
        .db_name(&db)
        .user(&user)
        .pass(&pass)
        .charset(charset)
        .dialect(Dialect::D3)
        .connect()
        .with_context(|| format!("connecting to {host}:{port}/{db}"))?;
    println!("connect: ok in {:?}", t0.elapsed());

    report_server(&mut conn)?;
    report_numerics(&mut conn)?;
    report_types(&mut conn)?;
    report_charsets(&mut conn)?;
    report_capability_gaps();
    if bulk_rows > 0 {
        report_streaming(&mut conn, bulk_rows)?;
    }
    Ok(())
}

/// Read-only reconnaissance of an arbitrary existing database — no fixture
/// tables assumed, nothing written. Used to validate the introspection
/// queries and streaming behaviour against a real production-shaped schema.
fn survey(port: u16) -> Result<()> {
    let host = env_or("FB_HOST", "localhost");
    let db = env_or("FB_DB", "fixture.fdb");
    let user = env_or("FB_USER", "SYSDBA");
    let pass = env_or("FB_PASS", "masterkey");
    let charset = charset_by_name(&env_or("FB_CHARSET", "WIN1252"))?;

    let t0 = Instant::now();
    let mut conn = rsfbclient::builder_pure_rust()
        .host(&host)
        .port(port)
        .db_name(&db)
        .user(&user)
        .pass(&pass)
        .charset(charset)
        .dialect(Dialect::D3)
        .connect()
        .with_context(|| format!("connecting to {host}:{port}/{db}"))?;
    println!("connect: ok in {:?}", t0.elapsed());

    report_server(&mut conn)?;
    report_numerics(&mut conn)?;
    report_db_settings(&mut conn)?;
    report_object_counts(&mut conn)?;
    report_capability_gaps();

    let table = env_or("FB_TABLE", "");
    if !table.is_empty() {
        report_table_scan(&mut conn, &table)?;
    }
    Ok(())
}

/// Database-level settings the UI has to display and respect.
fn report_db_settings(conn: &mut impl Queryable) -> Result<()> {
    println!("\n── database ──");
    let row: Option<(Option<String>, Option<i32>, Option<String>)> = conn.query_first(
        "select rdb$character_set_name, rdb$relation_id, rdb$description \
         from rdb$database",
        (),
    )?;
    if let Some((cs, _, _)) = row {
        println!("  default charset   {}", cs.unwrap_or_default().trim_end());
    }

    // ODS version and page size come from MON$DATABASE on 2.1+.
    let mon: Option<(Option<i32>, Option<i32>, Option<i32>, Option<i32>)> = conn
        .query_first(
            "select mon$ods_major, mon$ods_minor, mon$page_size, mon$sql_dialect \
             from mon$database",
            (),
        )
        .unwrap_or(None);
    if let Some((maj, min, page, dialect)) = mon {
        println!(
            "  ODS               {}.{}",
            maj.unwrap_or(0),
            min.unwrap_or(0)
        );
        println!("  page size         {}", page.unwrap_or(0));
        println!("  sql dialect       {}", dialect.unwrap_or(0));
    }
    Ok(())
}

/// The Explorer tree's node counts. Doubles as a timing check: on a large
/// schema these queries run every time a database is expanded.
fn report_object_counts(conn: &mut impl Queryable) -> Result<()> {
    println!("\n── object counts (Explorer tree) ──");
    let queries: &[(&str, &str)] = &[
        ("Domains", "select count(*) from rdb$fields where rdb$field_name not starting with 'RDB$'"),
        ("Tables", "select count(*) from rdb$relations where rdb$view_blr is null and (rdb$system_flag is null or rdb$system_flag = 0)"),
        ("Views", "select count(*) from rdb$relations where rdb$view_blr is not null and (rdb$system_flag is null or rdb$system_flag = 0)"),
        ("Procedures", "select count(*) from rdb$procedures where (rdb$system_flag is null or rdb$system_flag = 0)"),
        ("Triggers", "select count(*) from rdb$triggers where (rdb$system_flag is null or rdb$system_flag = 0)"),
        ("Generators", "select count(*) from rdb$generators where (rdb$system_flag is null or rdb$system_flag = 0)"),
        ("Exceptions", "select count(*) from rdb$exceptions"),
        ("UDFs", "select count(*) from rdb$functions where (rdb$system_flag is null or rdb$system_flag = 0)"),
        ("Roles", "select count(*) from rdb$roles"),
        ("Indices", "select count(*) from rdb$indices where (rdb$system_flag is null or rdb$system_flag = 0)"),
    ];

    let total = Instant::now();
    for (label, sql) in queries {
        let t = Instant::now();
        match conn.query_first::<_, (i64,)>(sql, ()) {
            Ok(Some((n,))) => println!("  {label:<12} {n:>6}   ({:?})", t.elapsed()),
            Ok(None) => println!("  {label:<12} <no row>"),
            Err(e) => println!("  {label:<12} error: {e}"),
        }
    }
    println!("  ── all counts in {:?}", total.elapsed());
    Ok(())
}

/// Stream a real table: column types, first-row latency, throughput.
fn report_table_scan(conn: &mut impl Queryable, table: &str) -> Result<()> {
    println!("\n── scan {table} ──");
    let limit: usize = env_or("FB_SCAN_LIMIT", "50000").parse().unwrap_or(50_000);
    // FB_SCAN_SQL lets the spike compare a narrow projection against `select *`
    // on the same table, which is how column count was tied to the read-path bug.
    let sql = match env_or("FB_SCAN_SQL", "") {
        s if s.is_empty() => format!("select * from {table}"),
        s => s,
    };
    println!("  sql: {sql}");

    let t0 = Instant::now();
    let mut iter = conn.query_iter::<_, Row>(&sql, ())?;
    let prepare_and_execute = t0.elapsed();

    let mut count = 0usize;
    let mut first_row_at = None;
    let mut printed_cols = false;
    let mut mangled = 0usize;
    // Content fingerprint, comparable against another driver reading the
    // same query: text characters, binary bytes, integer sum, null count.
    let (mut text_chars, mut bin_bytes, mut int_sum, mut nulls) = (0usize, 0usize, 0i128, 0usize);
    // Exact sum of NUMERIC / DECIMAL values, held at DEC_SUM_SCALE digits.
    // None once it overflows: 38-digit values do not fit at that scale.
    let mut dec_sum = Some(0i128);

    while let Some(row) = iter.next() {
        let row = row?;
        if count == 0 {
            first_row_at = Some(t0.elapsed());
        }
        if !printed_cols {
            for col in &row.cols {
                println!("  {:<28} raw_type={:<4} {}", col.name, col.raw_type, describe(&col.value));
            }
            printed_cols = true;
        }
        // U+FFFD anywhere means the connection charset is wrong for this data.
        for col in &row.cols {
            match &col.value {
                SqlType::Text(t) => {
                    if t.contains('\u{FFFD}') {
                        mangled += 1;
                    }
                    text_chars += t.chars().count();
                }
                SqlType::Binary(b) => bin_bytes += b.len(),
                SqlType::Integer(i) => int_sum += *i as i128,
                SqlType::Decimal(d) => {
                    let unit = 10i128.pow((DEC_SUM_SCALE - d.scale().min(DEC_SUM_SCALE)) as u32);
                    dec_sum = dec_sum.and_then(|s| s.checked_add(d.mantissa().checked_mul(unit)?));
                }
                SqlType::Null => nulls += 1,
                _ => {}
            }
        }
        count += 1;
        if count >= limit {
            break;
        }
    }
    let elapsed = t0.elapsed();

    println!("  prepare+execute {prepare_and_execute:?}");
    println!("  first row       {:?}", first_row_at.unwrap_or_default());
    println!("  {count} rows in  {elapsed:?}");
    if elapsed.as_secs_f64() > 0.0 {
        println!("  throughput      {:.0} rows/s", count as f64 / elapsed.as_secs_f64());
    }
    println!("  replacement chars in {mangled} cells (0 = charset ok)");
    println!("  fingerprint     rows={count} text_chars={text_chars} binary_bytes={bin_bytes} int_sum={int_sum} dec_sum={} nulls={nulls}",
        dec_sum.map_or("overflow".to_string(), |s| Decimal::new(s, DEC_SUM_SCALE).to_string()));
    Ok(())
}

/// Server version and dialect, straight from the engine.
fn report_server(conn: &mut impl Queryable) -> Result<()> {
    println!("\n── server ──");
    for key in ["ENGINE_VERSION", "DB_NAME", "NETWORK_PROTOCOL", "CLIENT_ADDRESS"] {
        // 2.5 lacks some of these keys; a missing one is information, not an error.
        let got: Option<(Option<String>,)> = conn
            .query_first(
                "select rdb$get_context('SYSTEM', ?) from rdb$database",
                (key.to_string(),),
            )
            .unwrap_or(None);
        match got.and_then(|r| r.0) {
            Some(v) => println!("  {key:<17} {v}"),
            None => println!("  {key:<17} <unavailable>"),
        }
    }
    Ok(())
}

/// NUMERIC / DECIMAL must come back exactly, never through a float, and the
/// Firebird 4 types must read at all. Needs no tables, so it runs anywhere.
fn report_numerics(conn: &mut impl Queryable) -> Result<()> {
    println!("\n── exact numerics ──");
    let version: Option<(Option<String>,)> = conn
        .query_first("select rdb$get_context('SYSTEM', 'ENGINE_VERSION') from rdb$database", ())
        .unwrap_or(None);
    let major: u32 = version
        .and_then(|v| v.0)
        .and_then(|v| v.split('.').next()?.parse().ok())
        .unwrap_or(0);

    // (expression, what it must read back as, first Firebird major with the type)
    let cases = [
        ("cast(123.45 as numeric(9,2))", "123.45", 2),
        ("cast(-0.005 as numeric(4,3))", "-0.005", 2),
        ("cast(0.1 as numeric(18,2)) + cast(0.2 as numeric(18,2))", "0.30", 2),
        ("cast(-922337203685477.5808 as numeric(18,4))", "-922337203685477.5808", 2),
        ("cast(-1 as int128)", "-1", 4),
        ("cast('123456789012345678901234567890.12' as numeric(38,2))", "123456789012345678901234567890.12", 4),
        ("cast('1.5E300' as decfloat(34))", "1.5E+300", 4),
        ("timestamp '2026-10-26 10:00:00 America/Sao_Paulo'", "2026-10-26 10:00:00.0000 America/Sao_Paulo", 4),
    ];
    let mut failed = 0;
    for (expr, want, since) in cases {
        if major < since {
            continue;
        }
        let got = conn
            .query_first::<_, Row>(&format!("select {expr} from rdb$database"), ())
            .map_err(anyhow::Error::from)
            .and_then(|row| row.context("no row")?.cols.into_iter().next().context("no column"))
            .map(|col| shown(&col.value));
        report_case(expr, want, got, &mut failed);
    }

    // Parameters: a BIGINT-sized decimal goes as a scaled INT64, a wider one as text.
    let params = [
        ("numeric(18,4)", "-12345.6789", 2),
        ("numeric(38,6)", "-98765432109876543210.123456", 4),
    ];
    for (ty, value, since) in params {
        if major < since {
            continue;
        }
        let sent: Decimal = value.parse()?;
        let got = conn
            .query_first::<_, Row>(&format!("select cast(? as {ty}) from rdb$database"), (sent,))
            .map_err(anyhow::Error::from)
            .and_then(|row| row.context("no row")?.cols.into_iter().next().context("no column"))
            .map(|col| shown(&col.value));
        report_case(&format!("? as {ty}"), value, got, &mut failed);
    }

    if failed > 0 {
        anyhow::bail!("{failed} exact-numeric checks failed");
    }
    Ok(())
}

fn report_case(what: &str, want: &str, got: Result<String>, failed: &mut usize) {
    match got {
        Ok(got) if got == want => println!("  ok    {what} = {got}"),
        Ok(got) => {
            *failed += 1;
            println!("  FAIL  {what} = {got}, want {want}");
        }
        Err(e) => {
            *failed += 1;
            println!("  FAIL  {what}: {e}");
        }
    }
}

/// A value as the grid would print it
fn shown(v: &SqlType) -> String {
    match v {
        SqlType::Text(s) => s.clone(),
        SqlType::Decimal(d) => d.to_string(),
        other => describe(other),
    }
}

/// Dump every column of the fixture table with its raw Firebird type code and
/// the Rust value it mapped to. This is the type-mapping table the grid needs.
fn report_types(conn: &mut impl Queryable) -> Result<()> {
    println!("\n── type mapping (TIPOS) ──");
    let rows: Vec<Row> = conn.query("select * from TIPOS order by ID", ())?;
    let Some(first) = rows.first() else {
        println!("  no rows — is the fixture seeded?");
        return Ok(());
    };

    for col in &first.cols {
        println!(
            "  {:<16} raw_type={:<4} {}",
            col.name,
            col.raw_type,
            describe(&col.value)
        );
    }
    println!("  ({} rows total)", rows.len());
    Ok(())
}

/// The WIN1252-vs-UTF8 tripwire. Legacy Brazilian databases are almost all
/// WIN1252, and silent mojibake here would poison every grid cell.
fn report_charsets(conn: &mut impl Queryable) -> Result<()> {
    println!("\n── charset round-trip ──");
    let row: Option<(Option<String>, Option<String>)> = conn.query_first(
        "select C_VC_WIN1252, C_VC_UTF8 from TIPOS where ID = 1",
        (),
    )?;

    match row {
        Some((win, utf)) => {
            check("WIN1252 column", win.as_deref());
            check("UTF8 column", utf.as_deref());
        }
        None => println!("  fixture row ID=1 missing"),
    }
    Ok(())
}

fn check(label: &str, got: Option<&str>) {
    match got {
        Some(s) if s == EXPECTED_ACCENTED => println!("  {label:<16} ok  {s:?}"),
        Some(s) => println!("  {label:<16} MANGLED got {s:?} want {EXPECTED_ACCENTED:?}"),
        None => println!("  {label:<16} null"),
    }
}

/// Streaming throughput at several fetch batch sizes. The driver batches
/// op_fetch internally (200 rows by default, FB_FETCH_BATCH overrides), so
/// this measures whether our chunked-grid design needs a driver change.
fn report_streaming(conn: &mut impl Queryable, limit: usize) -> Result<()> {
    println!("\n── streaming {limit} rows ──");
    let sql = "select ID, NOME, VALOR, CRIADO_EM from BULK_ROWS";

    let t0 = Instant::now();
    let mut count = 0usize;
    let mut first_row_at = None;
    for row in conn.query_iter::<_, Row>(sql, ())? {
        let _row = row?;
        if count == 0 {
            first_row_at = Some(t0.elapsed());
        }
        count += 1;
        if count >= limit {
            break;
        }
    }
    let elapsed = t0.elapsed();

    println!("  batch size      {}", env_or("FB_FETCH_BATCH", "200 (default)"));
    println!("  first row       {:?}", first_row_at.unwrap_or_default());
    println!("  {count} rows in  {elapsed:?}");
    if elapsed.as_secs_f64() > 0.0 {
        println!("  throughput      {:.0} rows/s", count as f64 / elapsed.as_secs_f64());
    }
    Ok(())
}

/// Capabilities the driver does not have. Confirmed by source audit; printed
/// here so the spike output is a complete picture rather than half of one.
fn report_capability_gaps() {
    println!("\n── known driver gaps (fb-wire) ──");
    for (what, blocks) in [
        ("op_cancel", "stopping a running query"),
        ("Services API (op_service_*)", "backup, restore, validate, stats, users"),
        ("events on the pure-Rust backend", "POST_EVENT listeners"),
    ] {
        println!("  {what:<38} blocks: {blocks}");
    }
}

/// Scale of the scan fingerprint's decimal sum: wider than any column here.
const DEC_SUM_SCALE: u8 = 10;

fn describe(v: &SqlType) -> String {
    match v {
        SqlType::Text(s) => format!("Text({s:?})"),
        SqlType::Integer(i) => format!("Integer({i})"),
        SqlType::Floating(f) => format!("Floating({f})"),
        SqlType::Timestamp(t) => format!("Timestamp({t:?})"),
        SqlType::Null => "Null".to_string(),
        SqlType::Binary(b) => format!("Binary({} bytes)", b.len()),
        SqlType::Boolean(b) => format!("Boolean({b})"),
        SqlType::Decimal(d) => format!("Decimal({d})"),
    }
}

fn charset_by_name(name: &str) -> Result<Charset> {
    Ok(match name.to_ascii_uppercase().as_str() {
        "UTF8" | "UTF-8" => rsfbclient::charset::UTF_8,
        "WIN1252" | "WIN_1252" => rsfbclient::charset::WIN_1252,
        "ISO8859_1" | "ISO_8859_1" => rsfbclient::charset::ISO_8859_1,
        "ASCII" | "NONE" => rsfbclient::charset::ASCII,
        other => anyhow::bail!("unknown charset {other}"),
    })
}

fn env_or(key: &str, default: &str) -> String {
    env::var(key).unwrap_or_else(|_| default.to_string())
}

fn arg_value<T: std::str::FromStr>(args: &[String], flag: &str) -> Option<T> {
    let i = args.iter().position(|a| a == flag)?;
    args.get(i + 1)?.parse().ok()
}
