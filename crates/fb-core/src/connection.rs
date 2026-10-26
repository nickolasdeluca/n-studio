use rsfbclient_core::{
    FirebirdClientDbOps, FirebirdClientSqlOps, FreeStmtOp, SqlType, StmtType, TrDataAccessMode, TrIsolationLevel, TrOp,
    TrRecordVersion, TransactionConfiguration,
};
use rsfbclient_rust::{CancelHandle, DatabaseCounters, RustFbClient, RustFbClientAttachmentConfig};
use serde::Serialize;
use std::collections::HashMap;
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Instant;

use crate::editor::{
    ColumnInfo, CursorId, ExecuteOptions, Executed, Fetched, Performance, StatementType, TxEnd, TxId, TxOptions,
};
use crate::error::{Error, Result};
use crate::params::{charset_by_name, ConnectParams, Dialect};
use crate::value::Value;

/// A live attachment, driven by its own thread. Dropping it detaches.
pub struct Connection {
    tx: Sender<Request>,
    info: ServerInfo,
    cancel: Option<CancelHandle>,
    /// Whether the connection's thread is running a request: a cancel sent
    /// when nothing runs would be held by the server for the next one
    busy: Arc<Mutex<bool>>,
}

/// What the server and the database say about themselves on connect
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
pub struct ServerInfo {
    /// As the server reports it, e.g. "4.0.4"
    pub engine_version: String,
    pub major: u32,
    pub minor: u32,
    /// On-disk structure version: 11 is 2.x, 12 is 3.0, 13 is 4.0 and 5.0
    pub ods_major: u32,
    pub ods_minor: u32,
    pub page_size: u32,
    /// The database's dialect, which can differ from the connection's
    pub dialect: u32,
    /// The database's default character set
    pub charset: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct QueryResult {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Value>>,
}

type Reply<T> = Sender<Result<T>>;

enum Request {
    Query { sql: String, params: Vec<Value>, reply: Reply<QueryResult> },
    Begin { options: TxOptions, reply: Reply<TxId> },
    End { tx: TxId, how: TxEnd, reply: Reply<()> },
    Execute { tx: TxId, sql: String, options: ExecuteOptions, explained: bool, reply: Reply<Executed> },
    Fetch { cursor: CursorId, max: usize, reply: Reply<Fetched> },
    CursorPerformance { cursor: CursorId, reply: Reply<Option<Performance>> },
    CloseCursor { cursor: CursorId, reply: Reply<()> },
}

impl Connection {
    /// Connects and attaches. Blocks until the server answers or refuses.
    pub fn open(params: &ConnectParams) -> Result<Self> {
        let charset = charset_by_name(&params.charset).ok_or_else(|| Error::Client {
            message: format!("unknown connection character set {:?}", params.charset),
        })?;
        let config = RustFbClientAttachmentConfig {
            host: params.host.clone(),
            port: params.port,
            db_name: params.database.clone(),
            user: params.user.clone(),
            pass: params.password.clone(),
            role_name: params.role.clone().filter(|r| !r.trim().is_empty()),
            auth: params.auth,
        };
        let dialect = params.dialect;

        let (tx, rx) = channel();
        let (ready_tx, ready_rx) = channel();
        let busy = Arc::new(Mutex::new(false));
        let running = busy.clone();
        thread::Builder::new()
            .name(format!("fb {}:{}", params.host, params.database))
            .spawn(move || {
                let mut actor = match Actor::attach(charset, &config, dialect) {
                    Ok(actor) => {
                        let _ = ready_tx.send(Ok(actor.cli.cancel_handle()));
                        actor
                    }
                    Err(e) => {
                        let _ = ready_tx.send(Err(e));
                        return;
                    }
                };
                // Runs until the Connection is dropped and the channel closes.
                for request in rx {
                    *running.lock().unwrap_or_else(|e| e.into_inner()) = true;
                    match request {
                        Request::Query { sql, params, reply } => {
                            let _ = reply.send(actor.query(&sql, params));
                        }
                        Request::Begin { options, reply } => {
                            let _ = reply.send(actor.begin(options));
                        }
                        Request::End { tx, how, reply } => {
                            let _ = reply.send(actor.end(tx, how));
                        }
                        Request::Execute { tx, sql, options, explained, reply } => {
                            let _ = reply.send(actor.execute(tx, &sql, options, explained));
                        }
                        Request::Fetch { cursor, max, reply } => {
                            let _ = reply.send(actor.fetch(cursor, max));
                        }
                        Request::CursorPerformance { cursor, reply } => {
                            let _ = reply.send(actor.cursor_performance(cursor));
                        }
                        Request::CloseCursor { cursor, reply } => {
                            let _ = reply.send(actor.close_cursor(cursor));
                        }
                    }
                    *running.lock().unwrap_or_else(|e| e.into_inner()) = false;
                }
                actor.detach();
            })
            .map_err(|e| Error::Client { message: format!("cannot start a connection thread: {e}") })?;
        let cancel = ready_rx.recv().map_err(|_| Error::Closed)??;

        let mut conn = Connection { tx, info: ServerInfo::default(), cancel, busy };
        conn.info = conn.read_server_info()?;
        Ok(conn)
    }

    pub fn info(&self) -> &ServerInfo {
        &self.info
    }

    /// Runs `sql` in a short read-only transaction of its own and returns
    /// every row. For catalog reads and other small results, not for a user's
    /// query, which needs a cursor and a transaction it controls.
    pub fn query(&self, sql: &str, params: Vec<Value>) -> Result<QueryResult> {
        self.call(|reply| Request::Query { sql: sql.to_string(), params, reply })
    }

    /// Cancels the request the connection is running, from any thread: it
    /// fails with "operation was cancelled", and the transaction carries
    /// on. `false` when nothing was running. Firebird 2.5 and later.
    pub fn cancel(&self) -> Result<bool> {
        let cancel = self.cancel.as_ref().ok_or_else(|| Error::Client {
            message: "this server cannot cancel a running statement (Firebird 2.5 and later can)".into(),
        })?;
        // Held while sending, so the request is not marked done in between.
        // Its reply may still beat the cancel over the network; the server
        // then holds the cancel for the next request, which fails at once.
        let busy = self.busy.lock().unwrap_or_else(|e| e.into_inner());
        if !*busy {
            return Ok(false);
        }
        cancel.cancel()?;
        Ok(true)
    }

    /// Starts a transaction the caller ends with [`Connection::end`]. One
    /// left open is rolled back when the connection closes.
    pub fn begin(&self, options: TxOptions) -> Result<TxId> {
        self.call(|reply| Request::Begin { options, reply })
    }

    /// Commits or rolls back. Ending it for good closes its open cursors.
    pub fn end(&self, tx: TxId, how: TxEnd) -> Result<()> {
        self.call(|reply| Request::End { tx, how, reply })
    }

    /// Prepares and runs one statement in `tx`. A select leaves a cursor open
    /// to [`fetch`](Connection::fetch) from; a COMMIT or ROLLBACK ends `tx`.
    pub fn execute(&self, tx: TxId, sql: &str, options: ExecuteOptions) -> Result<Executed> {
        let explained = self.info.major >= 3;
        self.call(|reply| Request::Execute { tx, sql: sql.to_string(), options, explained, reply })
    }

    /// Up to `max` more rows. At the end of the rows the cursor closes.
    pub fn fetch(&self, cursor: CursorId, max: usize) -> Result<Fetched> {
        self.call(|reply| Request::Fetch { cursor, max, reply })
    }

    /// What an open cursor's statement has cost so far, if it was executed
    /// with `performance`
    pub fn cursor_performance(&self, cursor: CursorId) -> Result<Option<Performance>> {
        self.call(|reply| Request::CursorPerformance { cursor, reply })
    }

    /// Drops a cursor before its end. Closing one that is gone is not an
    /// error: it may have reached its end, or its transaction ended.
    pub fn close_cursor(&self, cursor: CursorId) -> Result<()> {
        self.call(|reply| Request::CloseCursor { cursor, reply })
    }

    fn call<T>(&self, request: impl FnOnce(Reply<T>) -> Request) -> Result<T> {
        let (reply, rx) = channel();
        self.tx.send(request(reply)).map_err(|_| Error::Closed)?;
        rx.recv().map_err(|_| Error::Closed)?
    }

    fn read_server_info(&self) -> Result<ServerInfo> {
        let result = self.query(
            "select rdb$get_context('SYSTEM', 'ENGINE_VERSION'), \
                    mon$ods_major, mon$ods_minor, mon$page_size, mon$sql_dialect, \
                    (select trim(rdb$character_set_name) from rdb$database) \
             from mon$database",
            vec![],
        )?;
        let row = result.rows.first().ok_or_else(|| Error::Client { message: "mon$database is empty".into() })?;
        let int = |i: usize| row.get(i).and_then(Value::as_i64).unwrap_or(0) as u32;
        let engine_version = row.first().and_then(Value::as_str).unwrap_or_default().to_string();
        let mut parts = engine_version.split('.').map(|p| p.parse().unwrap_or(0));
        Ok(ServerInfo {
            major: parts.next().unwrap_or(0),
            minor: parts.next().unwrap_or(0),
            engine_version,
            ods_major: int(1),
            ods_minor: int(2),
            page_size: int(3),
            dialect: int(4),
            // A database created without one reports null: that is NONE
            charset: row.get(5).and_then(Value::as_str).unwrap_or("NONE").to_string(),
        })
    }
}

/// Lives on the connection's thread
struct Actor {
    cli: RustFbClient,
    db: rsfbclient_rust::DbHandle,
    dialect: rsfbclient_core::Dialect,
    transactions: HashMap<TxId, rsfbclient_rust::TrHandle>,
    cursors: HashMap<CursorId, Cursor>,
    /// Ids are never reused, so a stale one cannot name something new
    next_id: u64,
    /// Relation ids to names, for the performance tables; refreshed when an id
    /// it does not know turns up
    relations: HashMap<u16, String>,
}

struct Cursor {
    tx: TxId,
    stmt: rsfbclient_rust::StmtHandleData,
    /// The counters before it executed, when it runs with `performance`
    before: Option<DatabaseCounters>,
}

impl Actor {
    fn attach(
        charset: rsfbclient_core::Charset,
        config: &RustFbClientAttachmentConfig,
        dialect: Dialect,
    ) -> Result<Self> {
        let mut cli = RustFbClient::new(charset);
        let dialect = dialect.into();
        let db = cli.attach_database(config, dialect, false)?;
        Ok(Self {
            cli,
            db,
            dialect,
            transactions: HashMap::new(),
            cursors: HashMap::new(),
            next_id: 1,
            relations: HashMap::new(),
        })
    }

    fn query(&mut self, sql: &str, params: Vec<Value>) -> Result<QueryResult> {
        // Read committed with record versions: sees the latest committed
        // catalog and never waits on another transaction's pending changes.
        let mut tr = self.cli.begin_transaction(
            &mut self.db,
            TransactionConfiguration {
                data_access: TrDataAccessMode::ReadOnly,
                isolation: TrIsolationLevel::ReadCommited(TrRecordVersion::RecordVersion),
                ..Default::default()
            },
        )?;
        let result = self.run(&mut tr, sql, params);
        // Nothing to keep from a read-only transaction; a failure to end it
        // matters only if the query itself succeeded.
        let ended = self.cli.transaction_operation(&mut tr, TrOp::Rollback);
        let result = result?;
        ended?;
        Ok(result)
    }

    fn run(&mut self, tr: &mut rsfbclient_rust::TrHandle, sql: &str, params: Vec<Value>) -> Result<QueryResult> {
        let (stmt_type, mut stmt) = self.cli.prepare_statement(&mut self.db, tr, self.dialect, sql)?;
        let columns = stmt.column_names().map(str::to_owned).collect();
        let params: Vec<SqlType> = params.into_iter().map(Into::into).collect();
        self.cli.execute(&mut self.db, tr, &mut stmt, params)?;

        let mut rows = Vec::new();
        if stmt_type == StmtType::Select {
            while let Some(row) = self.cli.fetch(&mut self.db, tr, &mut stmt)? {
                rows.push(row.into_iter().map(|c| c.value.into()).collect());
            }
        }
        self.cli.free_statement(&mut stmt, FreeStmtOp::Drop)?;
        Ok(QueryResult { columns, rows })
    }

    fn id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    fn begin(&mut self, options: TxOptions) -> Result<TxId> {
        let tr = self.cli.begin_transaction(&mut self.db, options.into())?;
        let id = self.id();
        self.transactions.insert(id, tr);
        Ok(id)
    }

    fn end(&mut self, tx: TxId, how: TxEnd) -> Result<()> {
        let tr = self.transactions.get_mut(&tx).ok_or_else(|| no_such("transaction"))?;
        if how.closes() {
            let ids: Vec<CursorId> = self.cursors.iter().filter(|(_, c)| c.tx == tx).map(|(id, _)| *id).collect();
            for id in ids {
                if let Some(mut c) = self.cursors.remove(&id) {
                    // The transaction's end frees them on the server anyway
                    let _ = self.cli.free_statement(&mut c.stmt, FreeStmtOp::Drop);
                }
            }
        }
        let op = match how {
            TxEnd::Commit => TrOp::Commit,
            TxEnd::CommitRetaining => TrOp::CommitRetaining,
            TxEnd::Rollback => TrOp::Rollback,
            TxEnd::RollbackRetaining => TrOp::RollbackRetaining,
        };
        // A failed commit leaves the transaction alive: it can still roll back
        self.cli.transaction_operation(tr, op)?;
        if how.closes() {
            self.transactions.remove(&tx);
        }
        Ok(())
    }

    fn execute(&mut self, tx: TxId, sql: &str, options: ExecuteOptions, explained: bool) -> Result<Executed> {
        let tr = self.transactions.get_mut(&tx).ok_or_else(|| no_such("transaction"))?;

        let started = Instant::now();
        let (stmt_type, mut stmt) = self.cli.prepare_statement(&mut self.db, tr, self.dialect, sql)?;
        let prepare_ms = ms(started);
        let statement_type = StatementType::from(stmt_type);

        let refuse = |message: &str| Error::Client { message: message.to_string() };
        let early = match statement_type {
            // Through the transaction's own handle, so its state stays known
            StatementType::Commit | StatementType::Rollback => None,
            StatementType::StartTransaction => Some(refuse(
                "SET TRANSACTION is not run from the editor: pick the isolation in the transaction toolbar",
            )),
            _ if stmt.param_count() > 0 => Some(refuse("statements with parameters are not supported yet")),
            _ => None,
        };
        if let Some(e) = early {
            let _ = self.cli.free_statement(&mut stmt, FreeStmtOp::Drop);
            return Err(e);
        }
        if let StatementType::Commit | StatementType::Rollback = statement_type {
            let _ = self.cli.free_statement(&mut stmt, FreeStmtOp::Drop);
            let retaining = sql
                .to_ascii_uppercase()
                .split(|c: char| !c.is_ascii_alphanumeric())
                .any(|w| w == "RETAIN" || w == "RETAINING");
            let how = match (statement_type, retaining) {
                (StatementType::Commit, false) => TxEnd::Commit,
                (StatementType::Commit, true) => TxEnd::CommitRetaining,
                (_, false) => TxEnd::Rollback,
                (_, true) => TxEnd::RollbackRetaining,
            };
            let started = Instant::now();
            self.end(tx, how)?;
            return Ok(Executed {
                statement_type,
                columns: vec![],
                cursor: None,
                rows: vec![],
                records: Default::default(),
                plan: None,
                explained_plan: None,
                prepare_ms,
                execute_ms: ms(started),
                ended: Some(how),
                performance: None,
            });
        }

        let dialect = self.dialect as u32;
        let columns: Vec<ColumnInfo> = stmt.columns().iter().map(|c| ColumnInfo::describe(c, dialect)).collect();
        let (plan, explained_plan) = if options.plan && statement_type.has_plan() {
            // A plan that cannot be had does not stop the statement
            self.cli.statement_plans(&mut stmt, explained).unwrap_or_default()
        } else {
            (None, None)
        };

        let result = self.run_prepared(tx, &mut stmt, statement_type, options);
        let (rows, execute_ms, before) = match result {
            Ok(r) => r,
            Err(e) => {
                let _ = self.cli.free_statement(&mut stmt, FreeStmtOp::Drop);
                return Err(e);
            }
        };
        let records = stmt.records().into();

        let mut executed = Executed {
            statement_type,
            columns,
            cursor: None,
            rows,
            records,
            plan,
            explained_plan,
            prepare_ms,
            execute_ms,
            ended: None,
            performance: None,
        };
        if statement_type.has_cursor() {
            let id = self.id();
            self.cursors.insert(id, Cursor { tx, stmt, before });
            executed.cursor = Some(id);
        } else {
            self.cli.free_statement(&mut stmt, FreeStmtOp::Drop)?;
            if let Some(before) = before {
                executed.performance = Some(self.performance_since(&before)?);
            }
        }
        Ok(executed)
    }

    /// Executes, with the counter snapshot before it when asked for
    fn run_prepared(
        &mut self,
        tx: TxId,
        stmt: &mut rsfbclient_rust::StmtHandleData,
        statement_type: StatementType,
        options: ExecuteOptions,
    ) -> Result<(Vec<Vec<Value>>, f64, Option<DatabaseCounters>)> {
        let before = if options.performance { Some(self.cli.database_counters(&mut self.db)?) } else { None };
        let tr = self.transactions.get_mut(&tx).ok_or_else(|| no_such("transaction"))?;
        let started = Instant::now();
        let mut rows = Vec::new();
        // A procedure with outputs answers with its row, in the same round trip
        if statement_type == StatementType::ExecProcedure && !stmt.columns().is_empty() {
            let row = self.cli.execute2(&mut self.db, tr, stmt, vec![])?;
            rows.push(row.into_iter().map(|c| c.value.into()).collect());
        } else {
            self.cli.execute(&mut self.db, tr, stmt, vec![])?;
        }
        Ok((rows, ms(started), before))
    }

    fn fetch(&mut self, cursor: CursorId, max: usize) -> Result<Fetched> {
        let c = self.cursors.get_mut(&cursor).ok_or_else(|| no_such("cursor"))?;
        let tr = self.transactions.get_mut(&c.tx).ok_or_else(|| no_such("transaction"))?;
        let started = Instant::now();
        let mut rows = Vec::with_capacity(max.min(4096));
        let mut eof = false;
        while rows.len() < max {
            match self.cli.fetch(&mut self.db, tr, &mut c.stmt) {
                Ok(Some(row)) => rows.push(row.into_iter().map(|c| c.value.into()).collect()),
                Ok(None) => {
                    eof = true;
                    break;
                }
                Err(e) => {
                    // A cursor that failed has nothing more to give
                    let mut c = self.cursors.remove(&cursor).expect("looked up above");
                    let _ = self.cli.free_statement(&mut c.stmt, FreeStmtOp::Drop);
                    return Err(e.into());
                }
            }
        }
        let fetch_ms = ms(started);
        let mut performance = None;
        if eof {
            let mut c = self.cursors.remove(&cursor).expect("looked up above");
            if let Some(before) = &c.before {
                performance = Some(self.performance_since(before)?);
            }
            self.cli.free_statement(&mut c.stmt, FreeStmtOp::Drop)?;
        }
        Ok(Fetched { rows, eof, fetch_ms, performance })
    }

    fn cursor_performance(&mut self, cursor: CursorId) -> Result<Option<Performance>> {
        let c = self.cursors.get(&cursor).ok_or_else(|| no_such("cursor"))?;
        match c.before.clone() {
            Some(before) => Ok(Some(self.performance_since(&before)?)),
            None => Ok(None),
        }
    }

    fn close_cursor(&mut self, cursor: CursorId) -> Result<()> {
        if let Some(mut c) = self.cursors.remove(&cursor) {
            self.cli.free_statement(&mut c.stmt, FreeStmtOp::Drop)?;
        }
        Ok(())
    }

    fn performance_since(&mut self, before: &DatabaseCounters) -> Result<Performance> {
        let after = self.cli.database_counters(&mut self.db)?;
        if after.tables.keys().any(|id| !self.relations.contains_key(id)) {
            self.relations = self
                .query("select rdb$relation_id, trim(rdb$relation_name) from rdb$relations", vec![])?
                .rows
                .into_iter()
                .filter_map(|row| Some((row.first()?.as_i64()? as u16, row.get(1)?.as_str()?.to_string())))
                .collect();
        }
        let relations = &self.relations;
        Ok(Performance::between(before, &after, |id| {
            relations.get(&id).cloned().unwrap_or_else(|| format!("relation {id}"))
        }))
    }

    fn detach(mut self) {
        // Best effort: the server reclaims everything when the socket closes.
        // Open transactions roll back; detaching with them open would fail.
        for (_, mut tr) in self.transactions.drain() {
            let _ = self.cli.transaction_operation(&mut tr, TrOp::Rollback);
        }
        let _ = self.cli.detach_database(&mut self.db);
    }
}

fn no_such(what: &str) -> Error {
    Error::Client { message: format!("no such {what}: it has ended or was closed") }
}

fn ms(since: Instant) -> f64 {
    since.elapsed().as_secs_f64() * 1000.0
}
