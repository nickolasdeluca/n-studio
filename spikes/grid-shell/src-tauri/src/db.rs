//! A database cursor: one connection actor per query, on its own thread.
//!
//! The thread owns the client, the attachment, a read-only transaction and the
//! statement; the UI side only holds a channel. This is the threading model the
//! plan settles for `fb-core`, reduced to what the spike needs.

use rsfbclient_core::{
    Dialect, FirebirdClientDbOps, FirebirdClientSqlOps, FreeStmtOp, SqlType, StmtType,
    TrDataAccessMode, TrOp, TransactionConfiguration,
};
use rsfbclient_rust::{DatabaseCounters, RustFbClient, RustFbClientAttachmentConfig};
use serde::Serialize;
use std::collections::BTreeMap;
use std::env;
use std::sync::mpsc::{channel, Sender};
use std::thread;
use std::time::{Duration, Instant};

use crate::cursor::Cell;

type Reply<T> = Sender<Result<T, String>>;

enum Req {
    Fetch(usize, Reply<Vec<Box<[Cell]>>>),
    Perf(Reply<PerfReport>),
}

pub struct DbCursor {
    tx: Sender<Req>,
}

/// The Performance Analysis block for the statement, so far.
#[derive(Serialize)]
pub struct PerfReport {
    pub plan: Option<String>,
    pub explained_plan: Option<String>,
    pub prepare_ms: f64,
    pub execute_ms: f64,
    pub fetch_ms: f64,
    pub rows: usize,
    pub reads: i64,
    pub writes: i64,
    pub fetches: i64,
    pub marks: i64,
    pub current_memory: i64,
    pub max_memory: i64,
    pub buffers: i64,
    /// (table, indexed reads, non-indexed reads)
    pub tables: Vec<(String, i64, i64)>,
}

impl DbCursor {
    /// Connects with the `FB_*` environment, prepares and executes `sql`, and
    /// returns its column names. Rows come later, through `fetch`.
    pub fn open(sql: String) -> Result<(Vec<String>, DbCursor), String> {
        let (tx, rx) = channel();
        let (ready_tx, ready_rx) = channel();

        thread::Builder::new()
            .name("db-cursor".into())
            .spawn(move || {
                let mut actor = match Actor::open(&sql) {
                    Ok((actor, columns)) => {
                        let _ = ready_tx.send(Ok(columns));
                        actor
                    }
                    Err(e) => {
                        let _ = ready_tx.send(Err(e));
                        return;
                    }
                };
                // Runs until the cursor is dropped and the channel closes.
                for req in rx {
                    match req {
                        Req::Fetch(n, reply) => {
                            let _ = reply.send(actor.fetch(n));
                        }
                        Req::Perf(reply) => {
                            let _ = reply.send(actor.perf());
                        }
                    }
                }
                actor.close();
            })
            .map_err(|e| e.to_string())?;

        let columns = ready_rx.recv().map_err(|e| e.to_string())??;
        Ok((columns, DbCursor { tx }))
    }

    pub fn fetch(&mut self, n: usize) -> Result<Vec<Box<[Cell]>>, String> {
        self.call(|reply| Req::Fetch(n, reply))
    }

    pub fn perf(&mut self) -> Result<PerfReport, String> {
        self.call(Req::Perf)
    }

    fn call<T>(&self, req: impl FnOnce(Reply<T>) -> Req) -> Result<T, String> {
        let (reply, rx) = channel();
        self.tx.send(req(reply)).map_err(|_| "db cursor thread is gone")?;
        rx.recv().map_err(|_| "db cursor thread is gone")?
    }
}

struct Actor {
    cli: RustFbClient,
    db: rsfbclient_rust::DbHandle,
    tr: rsfbclient_rust::TrHandle,
    stmt: rsfbclient_rust::StmtHandleData,
    is_select: bool,
    done: bool,
    plan: Option<String>,
    explained_plan: Option<String>,
    prepare: Duration,
    execute: Duration,
    fetch: Duration,
    rows: usize,
    before: DatabaseCounters,
    names: Option<BTreeMap<u16, String>>,
}

impl Actor {
    fn open(sql: &str) -> Result<(Self, Vec<String>), String> {
        let e = |e: rsfbclient_core::FbError| e.to_string();
        let var = |k: &str, d: &str| env::var(k).unwrap_or_else(|_| d.to_string());

        let config = RustFbClientAttachmentConfig {
            host: var("FB_HOST", "localhost"),
            port: var("FB_PORT", "3050").parse().map_err(|_| "FB_PORT is not a port")?,
            db_name: var("FB_DB", "fixture.fdb"),
            user: var("FB_USER", "SYSDBA"),
            pass: var("FB_PASS", "masterkey"),
            role_name: None,
            ..Default::default()
        };
        let dialect = Dialect::D3;

        let mut cli = RustFbClient::new(rsfbclient_core::charset::WIN_1252);
        let mut db = cli.attach_database(&config, dialect, false).map_err(e)?;
        // Read-only: the spike runs arbitrary SQL against real databases.
        let mut tr = cli
            .begin_transaction(
                &mut db,
                TransactionConfiguration {
                    data_access: TrDataAccessMode::ReadOnly,
                    ..Default::default()
                },
            )
            .map_err(e)?;

        let t = Instant::now();
        let (stmt_type, mut stmt) =
            cli.prepare_statement(&mut db, &mut tr, dialect, sql).map_err(e)?;
        let prepare = t.elapsed();

        let plan = cli.statement_plan(&mut stmt, false).map_err(e)?;
        let explained_plan = cli.statement_plan(&mut stmt, true).unwrap_or(None);
        let columns = stmt.column_names().map(str::to_owned).collect();

        let before = cli.database_counters(&mut db).map_err(e)?;

        let t = Instant::now();
        cli.execute(&mut db, &mut tr, &mut stmt, vec![]).map_err(e)?;
        let execute = t.elapsed();

        let is_select = stmt_type == StmtType::Select;
        Ok((
            Self {
                cli,
                db,
                tr,
                stmt,
                is_select,
                done: !is_select,
                plan,
                explained_plan,
                prepare,
                execute,
                fetch: Duration::ZERO,
                rows: 0,
                before,
                names: None,
            },
            columns,
        ))
    }

    fn fetch(&mut self, n: usize) -> Result<Vec<Box<[Cell]>>, String> {
        let mut out = Vec::with_capacity(n);
        let t = Instant::now();
        while out.len() < n && !self.done {
            match self
                .cli
                .fetch(&mut self.db, &mut self.tr, &mut self.stmt)
                .map_err(|e| e.to_string())?
            {
                Some(row) => out.push(row.into_iter().map(|c| display(c.value)).collect()),
                None => self.done = true,
            }
        }
        self.fetch += t.elapsed();
        self.rows += out.len();
        Ok(out)
    }

    fn perf(&mut self) -> Result<PerfReport, String> {
        let now = self.cli.database_counters(&mut self.db).map_err(|e| e.to_string())?;
        let cost = now.since(&self.before);

        if self.names.is_none() {
            self.names = Some(self.relation_names()?);
        }
        let names = self.names.as_ref().expect("just filled");

        let ms = |d: Duration| d.as_secs_f64() * 1000.0;
        Ok(PerfReport {
            plan: self.plan.clone(),
            explained_plan: self.explained_plan.clone(),
            prepare_ms: ms(self.prepare),
            execute_ms: ms(self.execute),
            fetch_ms: ms(self.fetch),
            rows: self.rows,
            reads: cost.reads,
            writes: cost.writes,
            fetches: cost.fetches,
            marks: cost.marks,
            current_memory: cost.current_memory,
            max_memory: cost.max_memory,
            buffers: cost.num_buffers,
            tables: cost
                .tables
                .iter()
                .map(|(id, t)| {
                    let name = names.get(id).cloned().unwrap_or_else(|| format!("#{id}"));
                    (name, t.idx_reads, t.seq_reads)
                })
                .collect(),
        })
    }

    /// Relation names by id. Read once, after the first counter snapshot that
    /// needs them, on a second statement in the same transaction.
    fn relation_names(&mut self) -> Result<BTreeMap<u16, String>, String> {
        let e = |e: rsfbclient_core::FbError| e.to_string();
        let (_, mut stmt) = self
            .cli
            .prepare_statement(
                &mut self.db,
                &mut self.tr,
                Dialect::D3,
                "select rdb$relation_id, trim(rdb$relation_name) from rdb$relations",
            )
            .map_err(e)?;
        self.cli.execute(&mut self.db, &mut self.tr, &mut stmt, vec![]).map_err(e)?;

        let mut names = BTreeMap::new();
        while let Some(row) = self.cli.fetch(&mut self.db, &mut self.tr, &mut stmt).map_err(e)? {
            if let [id, name] = &row[..] {
                if let (SqlType::Integer(id), SqlType::Text(name)) = (&id.value, &name.value) {
                    names.insert(*id as u16, name.clone());
                }
            }
        }
        self.cli.free_statement(&mut stmt, FreeStmtOp::Drop).map_err(e)?;
        Ok(names)
    }

    fn close(mut self) {
        // Best effort: the server reclaims all of it when the socket closes.
        if self.is_select {
            let _ = self.cli.free_statement(&mut self.stmt, FreeStmtOp::Drop);
        }
        let _ = self.cli.transaction_operation(&mut self.tr, TrOp::Rollback);
        let _ = self.cli.detach_database(&mut self.db);
    }
}

/// Grid text for a value. Numbers arrive as the driver decodes them; see
/// M0-FINDINGS on NUMERIC going through f64.
fn display(v: SqlType) -> Cell {
    let s = match v {
        SqlType::Null => return None,
        SqlType::Text(s) => s,
        SqlType::Integer(i) => i.to_string(),
        SqlType::Floating(f) => f.to_string(),
        SqlType::Decimal(d) => d.to_string(),
        SqlType::Timestamp(t) => t.to_string(),
        SqlType::Boolean(b) => b.to_string(),
        SqlType::Binary(b) => format!("(BLOB, {} bytes)", b.len()),
    };
    Some(s.into_boxed_str())
}
