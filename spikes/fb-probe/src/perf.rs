//! `--perf "<sql>"`: the M0 exit criterion. Runs one statement the way the SQL
//! editor will — prepare, plan, execute, fetch everything — and prints the
//! performance block IBExpert shows under Performance Analysis.
//!
//! Drives the wire client directly rather than through `rsfbclient::Connection`,
//! which keeps the statement handle (and so the plan) out of reach. This is the
//! shape `fb-core` will have.
//!
//! The transaction is read-only: whatever the SQL is, nothing gets written.

use anyhow::{Context, Result};
use rsfbclient_core::{
    Dialect, FirebirdClientDbOps, FirebirdClientSqlOps, FreeStmtOp, SqlType, StmtType,
    TrDataAccessMode, TrOp, TransactionConfiguration,
};
use rsfbclient_rust::{DatabaseCounters, RustFbClient, RustFbClientAttachmentConfig};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use crate::{charset_by_name, env_or};

pub fn perf(sql: &str) -> Result<()> {
    let config = RustFbClientAttachmentConfig {
        host: env_or("FB_HOST", "localhost"),
        port: env_or("FB_PORT", "3050").parse().context("FB_PORT")?,
        db_name: env_or("FB_DB", "fixture.fdb"),
        user: env_or("FB_USER", "SYSDBA"),
        pass: env_or("FB_PASS", "masterkey"),
        role_name: None,
    };
    let dialect = Dialect::D3;

    let mut cli = RustFbClient::new(charset_by_name(&env_or("FB_CHARSET", "WIN1252"))?);
    let mut db = cli
        .attach_database(&config, dialect, false)
        .with_context(|| format!("connecting to {}:{}/{}", config.host, config.port, config.db_name))?;

    let mut tr = cli.begin_transaction(
        &mut db,
        TransactionConfiguration {
            data_access: TrDataAccessMode::ReadOnly,
            ..Default::default()
        },
    )?;

    println!("sql: {sql}");

    let t = Instant::now();
    let (stmt_type, mut stmt) = cli.prepare_statement(&mut db, &mut tr, dialect, sql)?;
    let prepare = t.elapsed();

    let plan = cli.statement_plan(&mut stmt, false)?;
    // FB3+ only; 2.5 rejects the item, which is information, not a failure.
    let explained = cli.statement_plan(&mut stmt, true);

    let before = cli.database_counters(&mut db)?;

    let t = Instant::now();
    cli.execute(&mut db, &mut tr, &mut stmt, vec![])?;
    let execute = t.elapsed();

    let mut rows = 0usize;
    let mut fetch = Duration::ZERO;
    if stmt_type == StmtType::Select {
        loop {
            let t = Instant::now();
            let row = cli.fetch(&mut db, &mut tr, &mut stmt)?;
            fetch += t.elapsed();
            if row.is_none() {
                break;
            }
            rows += 1;
        }
    }

    let after = cli.database_counters(&mut db)?;
    cli.free_statement(&mut stmt, FreeStmtOp::Drop)?;

    let cost = after.since(&before);
    let names = relation_names(&mut cli, &mut db, &mut tr, dialect, &cost)?;

    cli.transaction_operation(&mut tr, TrOp::Rollback)?;
    cli.detach_database(&mut db)?;

    print_block(PerfRun {
        stmt_type,
        rows,
        prepare,
        execute,
        fetch,
        plan,
        explained: explained.map_err(|e| e.to_string()),
        cost,
        names,
    });
    Ok(())
}

struct PerfRun {
    stmt_type: StmtType,
    rows: usize,
    prepare: Duration,
    execute: Duration,
    fetch: Duration,
    plan: Option<String>,
    explained: Result<Option<String>, String>,
    cost: DatabaseCounters,
    names: BTreeMap<u16, String>,
}

fn print_block(r: PerfRun) {
    let c = &r.cost;

    println!("\n── Plan ──");
    println!("{}", r.plan.as_deref().map(str::trim).unwrap_or("<none>"));

    println!("\n── Explained plan ──");
    match &r.explained {
        Ok(Some(p)) => println!("{}", p.trim()),
        Ok(None) => println!("<none>"),
        Err(e) => println!("<unavailable: {e}>"),
    }

    println!("\n── Query time ──");
    println!("  statement type      {:?}", r.stmt_type);
    println!("  prepare             {:>10.3} ms", ms(r.prepare));
    println!("  execute             {:>10.3} ms", ms(r.execute));
    println!("  fetch ({} rows)  {:>10.3} ms", r.rows, ms(r.fetch));
    if r.rows > 0 {
        println!("  avg fetch time      {:>10.3} ms", ms(r.fetch) / r.rows as f64);
    }

    println!("\n── Memory ──");
    println!("  current             {:>14}", c.current_memory);
    println!("  max                 {:>14}", c.max_memory);
    println!("  buffers             {:>14}", c.num_buffers);

    println!("\n── Operations (this statement) ──");
    println!("  reads               {:>14}", c.reads);
    println!("  writes              {:>14}", c.writes);
    println!("  fetches             {:>14}", c.fetches);
    println!("  marks               {:>14}", c.marks);

    println!("\n── Per-table reads ──");
    if c.tables.is_empty() {
        println!("  <no table activity>");
    }
    println!("  {:<32} {:>12} {:>12}", "table", "indexed", "non-indexed");
    for (id, t) in &c.tables {
        let name = r.names.get(id).map(String::as_str).unwrap_or("?");
        println!("  {:<32} {:>12} {:>12}", format!("{name} ({id})"), t.idx_reads, t.seq_reads);
        let writes = [("inserts", t.inserts), ("updates", t.updates), ("deletes", t.deletes)];
        for (what, n) in writes.iter().filter(|(_, n)| *n != 0) {
            println!("    {what} {n}");
        }
    }
}

/// Names for the relations that showed up in the counters, read after the
/// "after" snapshot so this lookup does not count against the statement.
fn relation_names(
    cli: &mut RustFbClient,
    db: &mut rsfbclient_rust::DbHandle,
    tr: &mut rsfbclient_rust::TrHandle,
    dialect: Dialect,
    cost: &DatabaseCounters,
) -> Result<BTreeMap<u16, String>> {
    let mut names = BTreeMap::new();
    if cost.tables.is_empty() {
        return Ok(names);
    }

    let (_, mut stmt) = cli.prepare_statement(
        db,
        tr,
        dialect,
        "select rdb$relation_id, trim(rdb$relation_name) from rdb$relations",
    )?;
    cli.execute(db, tr, &mut stmt, vec![])?;
    while let Some(row) = cli.fetch(db, tr, &mut stmt)? {
        if let [id, name] = &row[..] {
            if let (SqlType::Integer(id), SqlType::Text(name)) = (&id.value, &name.value) {
                if cost.tables.contains_key(&(*id as u16)) {
                    names.insert(*id as u16, name.clone());
                }
            }
        }
    }
    cli.free_statement(&mut stmt, FreeStmtOp::Drop)?;
    Ok(names)
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}
