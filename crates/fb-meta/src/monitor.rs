//! What a database is doing now, from its monitoring tables (`MON$`): who is
//! attached, the transactions open, the statements prepared and running, and
//! the counters that say how far the oldest transaction lags.
//!
//! The server takes a snapshot of these tables the first time a transaction
//! reads one, so each reader here is one query in a transaction of its own:
//! reading again shows the present. Only SYSDBA and the database's owner see
//! every attachment; anyone else sees their own.

use fb_core::{Connection, ExecuteOptions, Result, TxEnd, TxOptions, Value};
use serde::Serialize;

use crate::ddl::{Context, Statement as DdlStatement};
use crate::row::{inline, rows, Row};
use crate::Generation;

/// A point in time as text, whatever type the version gives it (Firebird 4
/// has them with a time zone): to the second
fn moment(expr: &str) -> String {
    format!("cast(cast({expr} as timestamp) as varchar(30))")
}

fn seconds(text: Option<String>) -> Option<String> {
    text.map(|t| t.chars().take(19).collect())
}

/// The database's header counters
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DatabaseActivity {
    pub page_size: i64,
    pub ods: String,
    pub pages: i64,
    pub page_buffers: i64,
    pub dialect: i64,
    pub created: Option<String>,
    /// The oldest transaction that is not committed: garbage newer than it
    /// cannot be collected
    pub oldest_transaction: i64,
    pub oldest_active: i64,
    pub oldest_snapshot: i64,
    pub next_transaction: i64,
    pub sweep_interval: i64,
    pub read_only: bool,
    pub forced_writes: bool,
    pub reserve_space: bool,
    /// "online", "multi-user shutdown", "single-user shutdown", "full shutdown"
    pub shutdown_mode: &'static str,
    /// "normal", "stalled", "merge": where a physical backup (`nbackup`) is
    pub backup_state: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Attachment {
    pub id: i64,
    /// This connection: it is not to be killed from itself
    pub current: bool,
    /// The server's own: the garbage collector, the cache writer
    pub system: bool,
    pub active: bool,
    pub user: Option<String>,
    pub role: Option<String>,
    pub protocol: Option<String>,
    pub address: Option<String>,
    /// Firebird 3 and later
    pub host: Option<String>,
    /// Firebird 3 and later
    pub os_user: Option<String>,
    pub process: Option<String>,
    pub process_id: Option<i64>,
    /// Firebird 3 and later
    pub client_version: Option<String>,
    pub server_pid: Option<i64>,
    pub since: Option<String>,
    pub page_reads: i64,
    pub page_writes: i64,
    pub page_fetches: i64,
    pub page_marks: i64,
    pub seq_reads: i64,
    pub idx_reads: i64,
    pub inserts: i64,
    pub updates: i64,
    pub deletes: i64,
    pub memory_used: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Transaction {
    pub id: i64,
    pub attachment: i64,
    pub active: bool,
    pub since: Option<String>,
    pub isolation: &'static str,
    pub read_only: bool,
    pub auto_commit: bool,
    /// Seconds a lock is waited for: -1 for ever, 0 not at all
    pub lock_timeout: i64,
    pub oldest_transaction: i64,
    pub oldest_active: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Statement {
    pub id: i64,
    pub attachment: i64,
    pub transaction: Option<i64>,
    /// "idle", "active", "stalled" (a cursor open, waiting to be fetched)
    pub state: &'static str,
    pub since: Option<String>,
    /// The start of its text
    pub sql: Option<String>,
    pub cut: bool,
}

fn flag_or(row: &Row, i: usize) -> i64 {
    row.int(i).unwrap_or(0)
}

pub fn database(conn: &Connection) -> Result<Option<DatabaseActivity>> {
    let sql = format!(
        "select mon$page_size, mon$ods_major, mon$ods_minor, mon$pages, mon$page_buffers, mon$sql_dialect, {}, \
         mon$oldest_transaction, mon$oldest_active, mon$oldest_snapshot, mon$next_transaction, mon$sweep_interval, \
         mon$read_only, mon$forced_writes, mon$reserve_space, mon$shutdown_mode, mon$backup_state \
         from mon$database",
        moment("mon$creation_date")
    );
    Ok(rows(conn, &sql, vec![])?.first().map(|r| DatabaseActivity {
        page_size: flag_or(r, 0),
        ods: format!("{}.{}", flag_or(r, 1), flag_or(r, 2)),
        pages: flag_or(r, 3),
        page_buffers: flag_or(r, 4),
        dialect: flag_or(r, 5),
        created: seconds(r.text(6)),
        oldest_transaction: flag_or(r, 7),
        oldest_active: flag_or(r, 8),
        oldest_snapshot: flag_or(r, 9),
        next_transaction: flag_or(r, 10),
        sweep_interval: flag_or(r, 11),
        read_only: r.flag(12),
        forced_writes: r.flag(13),
        reserve_space: r.flag(14),
        shutdown_mode: match flag_or(r, 15) {
            0 => "online",
            1 => "multi-user shutdown",
            2 => "single-user shutdown",
            _ => "full shutdown",
        },
        backup_state: match flag_or(r, 16) {
            0 => "normal",
            1 => "stalled",
            _ => "merge",
        },
    }))
}

pub fn attachments(conn: &Connection) -> Result<Vec<Attachment>> {
    let later = match Generation::of(conn.info()) {
        Generation::Fb3 => "a.mon$system_flag, a.mon$remote_host, a.mon$remote_os_user, a.mon$client_version",
        Generation::Fb2 => "0, cast(null as varchar(1)), cast(null as varchar(1)), cast(null as varchar(1))",
    };
    let sql = format!(
        "select a.mon$attachment_id, iif(a.mon$attachment_id = current_connection, 1, 0), a.mon$state, \
         a.mon$user, a.mon$role, a.mon$remote_protocol, a.mon$remote_address, a.mon$remote_process, \
         a.mon$remote_pid, a.mon$server_pid, {since}, \
         io.mon$page_reads, io.mon$page_writes, io.mon$page_fetches, io.mon$page_marks, \
         r.mon$record_seq_reads, r.mon$record_idx_reads, r.mon$record_inserts, r.mon$record_updates, \
         r.mon$record_deletes, m.mon$memory_used, {later} \
         from mon$attachments a \
         left join mon$io_stats io on io.mon$stat_id = a.mon$stat_id \
         left join mon$record_stats r on r.mon$stat_id = a.mon$stat_id \
         left join mon$memory_usage m on m.mon$stat_id = a.mon$stat_id \
         order by a.mon$attachment_id",
        since = moment("a.mon$timestamp"),
    );
    Ok(rows(conn, &sql, vec![])?
        .iter()
        .map(|r| Attachment {
            id: flag_or(r, 0),
            current: r.flag(1),
            active: r.flag(2),
            user: r.text(3),
            // A connection without a role reports it as NONE
            role: r.text(4).filter(|role| role != "NONE"),
            protocol: r.text(5),
            address: r.text(6),
            process: r.text(7),
            process_id: r.int(8),
            server_pid: r.int(9),
            since: seconds(r.text(10)),
            page_reads: flag_or(r, 11),
            page_writes: flag_or(r, 12),
            page_fetches: flag_or(r, 13),
            page_marks: flag_or(r, 14),
            seq_reads: flag_or(r, 15),
            idx_reads: flag_or(r, 16),
            inserts: flag_or(r, 17),
            updates: flag_or(r, 18),
            deletes: flag_or(r, 19),
            memory_used: flag_or(r, 20),
            system: r.flag(21),
            host: r.text(22),
            os_user: r.text(23),
            client_version: r.text(24),
        })
        .collect())
}

pub fn transactions(conn: &Connection) -> Result<Vec<Transaction>> {
    let sql = format!(
        "select t.mon$transaction_id, t.mon$attachment_id, t.mon$state, {}, t.mon$isolation_mode, t.mon$read_only, \
         t.mon$auto_commit, t.mon$lock_timeout, t.mon$oldest_transaction, t.mon$oldest_active \
         from mon$transactions t order by t.mon$transaction_id",
        moment("t.mon$timestamp")
    );
    Ok(rows(conn, &sql, vec![])?
        .iter()
        .map(|r| Transaction {
            id: flag_or(r, 0),
            attachment: flag_or(r, 1),
            active: r.flag(2),
            since: seconds(r.text(3)),
            isolation: match flag_or(r, 4) {
                0 => "consistency",
                1 => "snapshot",
                2 => "read committed, record version",
                3 => "read committed, no record version",
                _ => "read committed, read consistency",
            },
            read_only: r.flag(5),
            auto_commit: r.flag(6),
            lock_timeout: r.int(7).unwrap_or(-1),
            oldest_transaction: flag_or(r, 8),
            oldest_active: flag_or(r, 9),
        })
        .collect())
}

/// The statements with a text. `running` leaves out the idle ones: prepared
/// and kept, of which an application may hold hundreds.
pub fn statements(conn: &Connection, running: bool) -> Result<Vec<Statement>> {
    let sql = format!(
        "select s.mon$statement_id, s.mon$attachment_id, s.mon$transaction_id, s.mon$state, {}, {} \
         from mon$statements s where s.mon$sql_text is not null{} \
         order by s.mon$state desc, s.mon$statement_id",
        moment("s.mon$timestamp"),
        inline("s.mon$sql_text"),
        if running { " and s.mon$state <> 0" } else { "" },
    );
    Ok(rows(conn, &sql, vec![])?
        .iter()
        .map(|r| {
            let (sql, cut) = r.lob(5);
            Statement {
                id: flag_or(r, 0),
                attachment: flag_or(r, 1),
                transaction: r.int(2),
                state: match flag_or(r, 3) {
                    0 => "idle",
                    1 => "active",
                    _ => "stalled",
                },
                since: seconds(r.text(4)),
                sql,
                cut,
            }
        })
        .collect())
}

/// The whole text of a statement, while it is still there
pub fn statement_text(conn: &Connection, id: i64) -> Result<Option<String>> {
    let found = rows(conn, "select mon$sql_text from mon$statements where mon$statement_id = ?", vec![Value::Integer(id)])?;
    Ok(found.first().and_then(|r| r.text(0)))
}

/// Disconnects an attachment, as the statement that asks it of the server:
/// its transactions are rolled back. Never the connection that runs it, nor
/// one of the server's own.
pub fn kill(id: i64, ctx: &Context) -> DdlStatement {
    let system = if ctx.major >= 3 { "\n  AND COALESCE(MON$SYSTEM_FLAG, 0) = 0" } else { "" };
    DdlStatement::plain(format!(
        "DELETE FROM MON$ATTACHMENTS\nWHERE MON$ATTACHMENT_ID = {id}\n  AND MON$ATTACHMENT_ID <> CURRENT_CONNECTION{system}"
    ))
}

/// Cancels a statement another attachment is running; that attachment and
/// its transaction carry on
pub fn cancel(id: i64) -> DdlStatement {
    DdlStatement::plain(format!(
        "DELETE FROM MON$STATEMENTS\nWHERE MON$STATEMENT_ID = {id}\n  AND MON$ATTACHMENT_ID <> CURRENT_CONNECTION"
    ))
}

/// Runs one of the statements above, in a transaction of its own
fn run(conn: &Connection, statement: &DdlStatement) -> Result<()> {
    let tx = conn.begin(TxOptions::default())?;
    let done = conn.execute(tx, &statement.sql, vec![], ExecuteOptions { plan: false, performance: false });
    match done {
        Ok(_) => conn.end(tx, TxEnd::Commit),
        Err(e) => {
            conn.end(tx, TxEnd::Rollback).ok();
            Err(e)
        }
    }
}

pub fn kill_attachment(conn: &Connection, id: i64) -> Result<()> {
    run(conn, &kill(id, &Context::of(conn.info())))
}

pub fn cancel_statement(conn: &Connection, id: i64) -> Result<()> {
    run(conn, &cancel(id))
}
