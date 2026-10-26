use rsfbclient_core::{
    FirebirdClientDbOps, FirebirdClientSqlOps, FreeStmtOp, SqlType, StmtType, TrDataAccessMode,
    TrIsolationLevel, TrOp, TrRecordVersion, TransactionConfiguration,
};
use rsfbclient_rust::{RustFbClient, RustFbClientAttachmentConfig};
use serde::Serialize;
use std::sync::mpsc::{channel, Sender};
use std::thread;

use crate::error::{Error, Result};
use crate::params::{charset_by_name, ConnectParams, Dialect};
use crate::value::Value;

/// A live attachment, driven by its own thread. Dropping it detaches.
pub struct Connection {
    tx: Sender<Request>,
    info: ServerInfo,
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
        thread::Builder::new()
            .name(format!("fb {}:{}", params.host, params.database))
            .spawn(move || {
                let mut actor = match Actor::attach(charset, &config, dialect) {
                    Ok(actor) => {
                        let _ = ready_tx.send(Ok(()));
                        actor
                    }
                    Err(e) => {
                        let _ = ready_tx.send(Err(e));
                        return;
                    }
                };
                // Runs until the Connection is dropped and the channel closes.
                for request in rx {
                    match request {
                        Request::Query { sql, params, reply } => {
                            let _ = reply.send(actor.query(&sql, params));
                        }
                    }
                }
                actor.detach();
            })
            .map_err(|e| Error::Client { message: format!("cannot start a connection thread: {e}") })?;
        ready_rx.recv().map_err(|_| Error::Closed)??;

        let mut conn = Connection { tx, info: ServerInfo::default() };
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
        Ok(Self { cli, db, dialect })
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

    fn detach(mut self) {
        // Best effort: the server reclaims everything when the socket closes.
        let _ = self.cli.detach_database(&mut self.db);
    }
}
