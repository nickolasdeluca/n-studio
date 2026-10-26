//! What an SQL editor works with: its own transaction, statements run in it,
//! cursors fetched a page at a time, and what each statement cost.

use rsfbclient_core::{
    ibase, StmtType, TrDataAccessMode, TrIsolationLevel, TrLockResolution, TrRecordVersion, TransactionConfiguration,
};
use rsfbclient_rust::{ColumnDesc, DatabaseCounters, RecordCounts};
use serde::{Deserialize, Serialize};

use crate::value::Value;

/// An editor's transaction, by the id `Connection::begin` gave it
pub type TxId = u64;
/// An open result set, by the id `Connection::execute` gave it
pub type CursorId = u64;

/// The transaction isolation level, as IBExpert's TIL selector names them
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Isolation {
    /// Sees what others commit; never waits on a newer record version
    #[default]
    ReadCommitted,
    /// Read committed that waits on (or, without wait, fails at) an
    /// uncommitted record version
    ReadCommittedNoRecVersion,
    /// One consistent view from the transaction's start
    Snapshot,
    /// Snapshot, and other transactions cannot write the tables it reads
    SnapshotTableStability,
}

/// What a transaction does when a record it needs is locked
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum LockWait {
    #[default]
    Wait,
    NoWait,
    Timeout {
        seconds: u32,
    },
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TxOptions {
    pub isolation: Isolation,
    pub read_only: bool,
    pub lock_wait: LockWait,
}

impl From<TxOptions> for TransactionConfiguration {
    fn from(o: TxOptions) -> Self {
        TransactionConfiguration {
            data_access: if o.read_only { TrDataAccessMode::ReadOnly } else { TrDataAccessMode::ReadWrite },
            isolation: match o.isolation {
                Isolation::ReadCommitted => TrIsolationLevel::ReadCommited(TrRecordVersion::RecordVersion),
                Isolation::ReadCommittedNoRecVersion => {
                    TrIsolationLevel::ReadCommited(TrRecordVersion::NoRecordVersion)
                }
                Isolation::Snapshot => TrIsolationLevel::Concurrency,
                Isolation::SnapshotTableStability => TrIsolationLevel::Consistency,
            },
            lock_resolution: match o.lock_wait {
                LockWait::Wait => TrLockResolution::Wait(None),
                LockWait::NoWait => TrLockResolution::NoWait,
                LockWait::Timeout { seconds } => TrLockResolution::Wait(Some(seconds)),
            },
        }
    }
}

/// How a transaction ends
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TxEnd {
    Commit,
    /// Commits and keeps the transaction (and its cursors) open
    CommitRetaining,
    Rollback,
    RollbackRetaining,
}

impl TxEnd {
    /// Whether the transaction is over afterwards
    pub fn closes(self) -> bool {
        matches!(self, TxEnd::Commit | TxEnd::Rollback)
    }
}

/// What kind of statement ran
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StatementType {
    Select,
    SelectForUpdate,
    Insert,
    Update,
    Delete,
    Ddl,
    ExecProcedure,
    SetGenerator,
    Savepoint,
    Commit,
    Rollback,
    StartTransaction,
    Other,
}

impl StatementType {
    /// Leaves a cursor open to fetch from
    pub fn has_cursor(self) -> bool {
        matches!(self, StatementType::Select | StatementType::SelectForUpdate)
    }

    /// Has an access plan to show
    pub fn has_plan(self) -> bool {
        matches!(
            self,
            StatementType::Select
                | StatementType::SelectForUpdate
                | StatementType::Insert
                | StatementType::Update
                | StatementType::Delete
                | StatementType::ExecProcedure
        )
    }
}

impl From<StmtType> for StatementType {
    fn from(t: StmtType) -> Self {
        match t {
            StmtType::Select => StatementType::Select,
            StmtType::SelectForUpd => StatementType::SelectForUpdate,
            StmtType::Insert => StatementType::Insert,
            StmtType::Update => StatementType::Update,
            StmtType::Delete => StatementType::Delete,
            StmtType::Ddl => StatementType::Ddl,
            StmtType::ExecProcedure => StatementType::ExecProcedure,
            StmtType::SetGenerator => StatementType::SetGenerator,
            StmtType::Savepoint => StatementType::Savepoint,
            StmtType::Commit => StatementType::Commit,
            StmtType::Rollback => StatementType::Rollback,
            StmtType::StartTrans => StatementType::StartTransaction,
            _ => StatementType::Other,
        }
    }
}

/// How a grid should treat a column's values
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ColumnKind {
    Integer,
    /// Exact: NUMERIC, DECIMAL, INT128, DECFLOAT
    Decimal,
    Float,
    Text,
    Date,
    Time,
    Timestamp,
    Boolean,
    TextBlob,
    Blob,
    Other,
}

impl ColumnKind {
    /// Numbers line up on the right
    pub fn numeric(self) -> bool {
        matches!(self, ColumnKind::Integer | ColumnKind::Decimal | ColumnKind::Float)
    }
}

/// An output column, as the "Query columns" tab lists it
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ColumnInfo {
    /// The alias, which is what the grid heads it with
    pub name: String,
    /// The table column it comes from, if any
    pub field: String,
    pub relation: String,
    /// As it would be declared: `NUMERIC(18,4)`, `VARCHAR(60)`, …
    pub type_name: String,
    pub kind: ColumnKind,
    pub nullable: bool,
}

impl ColumnInfo {
    pub(crate) fn describe(c: &ColumnDesc, dialect: u32) -> Self {
        let (type_name, kind) = type_of(c, dialect);
        Self {
            name: c.alias_name.clone(),
            field: c.field_name.clone(),
            relation: c.relation_name.clone(),
            type_name,
            kind,
            nullable: c.nullable,
        }
    }
}

fn type_of(c: &ColumnDesc, dialect: u32) -> (String, ColumnKind) {
    use ColumnKind::*;
    let scaled = |base: &str, precision: u32| -> (String, ColumnKind) {
        match c.scale {
            0 if c.subtype == 0 => (base.to_string(), Integer),
            0 => (format!("{}({precision})", decimal_name(c.subtype)), Decimal),
            s => (format!("{}({precision},{})", decimal_name(c.subtype), -s), Decimal),
        }
    };
    match c.sqltype as u32 {
        ibase::SQL_SHORT => scaled("SMALLINT", 4),
        ibase::SQL_LONG => scaled("INTEGER", 9),
        ibase::SQL_INT64 => scaled("BIGINT", 18),
        ibase::SQL_INT128 => scaled("INT128", 38),
        ibase::SQL_FLOAT => ("FLOAT".into(), Float),
        // Dialect 1 calls a DOUBLE with a scale NUMERIC too
        ibase::SQL_DOUBLE if c.scale != 0 && dialect == 1 => (format!("NUMERIC(15,{})", -c.scale), Float),
        ibase::SQL_DOUBLE => ("DOUBLE PRECISION".into(), Float),
        ibase::SQL_DEC16 => ("DECFLOAT(16)".into(), Decimal),
        ibase::SQL_DEC34 => ("DECFLOAT(34)".into(), Decimal),
        ibase::SQL_TEXT => (format!("CHAR({})", chars(c)), Text),
        ibase::SQL_VARYING => (format!("VARCHAR({})", chars(c)), Text),
        ibase::SQL_TYPE_DATE => ("DATE".into(), Date),
        ibase::SQL_TYPE_TIME => ("TIME".into(), Time),
        // In dialect 1 a DATE is a timestamp
        ibase::SQL_TIMESTAMP if dialect == 1 => ("DATE".into(), Timestamp),
        ibase::SQL_TIMESTAMP => ("TIMESTAMP".into(), Timestamp),
        ibase::SQL_TIME_TZ | ibase::SQL_TIME_TZ_EX => ("TIME WITH TIME ZONE".into(), Text),
        ibase::SQL_TIMESTAMP_TZ | ibase::SQL_TIMESTAMP_TZ_EX => ("TIMESTAMP WITH TIME ZONE".into(), Text),
        ibase::SQL_BOOLEAN => ("BOOLEAN".into(), Boolean),
        ibase::SQL_BLOB if c.subtype == 1 => ("BLOB SUB_TYPE TEXT".into(), TextBlob),
        ibase::SQL_BLOB => (format!("BLOB SUB_TYPE {}", c.subtype), Blob),
        ibase::SQL_NULL => ("NULL".into(), Other),
        t => (format!("type {t}"), Other),
    }
}

/// A scaled integer's subtype says how it was declared
fn decimal_name(subtype: i16) -> &'static str {
    if subtype == 2 {
        "DECIMAL"
    } else {
        "NUMERIC"
    }
}

/// A text column's length in characters: the server sends bytes, and the
/// subtype carries the character set
fn chars(c: &ColumnDesc) -> i64 {
    let bytes_per_char = match c.subtype & 0xff {
        3 => 3,                         // UNICODE_FSS
        4 => 4,                         // UTF8
        5 | 6 | 44 | 56 | 57 | 64 => 2, // SJIS, EUCJ, KSC, GB, BIG5, …
        _ => 1,
    };
    c.length as i64 / bytes_per_char
}

/// What one statement did
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Executed {
    pub statement_type: StatementType,
    pub columns: Vec<ColumnInfo>,
    /// Open for a select: fetch from it, then close it
    pub cursor: Option<CursorId>,
    /// An EXECUTE PROCEDURE's output row
    pub rows: Vec<Vec<Value>>,
    pub records: Records,
    pub plan: Option<String>,
    /// Firebird 3+
    pub explained_plan: Option<String>,
    pub prepare_ms: f64,
    pub execute_ms: f64,
    /// A COMMIT or ROLLBACK typed in the editor ends its transaction
    pub ended: Option<TxEnd>,
    /// Without a cursor, the counters are final already
    pub performance: Option<Performance>,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Records {
    pub selected: i64,
    pub inserted: i64,
    pub updated: i64,
    pub deleted: i64,
}

impl From<RecordCounts> for Records {
    fn from(r: RecordCounts) -> Self {
        Self { selected: r.selected, inserted: r.inserted, updated: r.updated, deleted: r.deleted }
    }
}

/// A page of rows from a cursor
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Fetched {
    pub rows: Vec<Vec<Value>>,
    /// The cursor has no more rows (and is closed)
    pub eof: bool,
    pub fetch_ms: f64,
    /// At the end, what the whole statement cost, if it ran with
    /// `performance`
    pub performance: Option<Performance>,
}

/// IBExpert's Performance Analysis: the attachment's counters since the
/// statement started
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Performance {
    pub reads: i64,
    pub writes: i64,
    pub fetches: i64,
    pub marks: i64,
    pub current_memory: i64,
    pub max_memory: i64,
    pub buffers: i64,
    pub page_size: i64,
    pub tables: Vec<TablePerformance>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TablePerformance {
    pub table: String,
    pub indexed_reads: i64,
    pub non_indexed_reads: i64,
    pub inserts: i64,
    pub updates: i64,
    pub deletes: i64,
    pub backouts: i64,
    pub purges: i64,
    pub expunges: i64,
}

impl Performance {
    pub(crate) fn between(
        before: &DatabaseCounters,
        after: &DatabaseCounters,
        table_name: impl Fn(u16) -> String,
    ) -> Self {
        let d = after.since(before);
        let mut tables: Vec<TablePerformance> = d
            .tables
            .iter()
            .map(|(id, t)| TablePerformance {
                table: table_name(*id),
                indexed_reads: t.idx_reads,
                non_indexed_reads: t.seq_reads,
                inserts: t.inserts,
                updates: t.updates,
                deletes: t.deletes,
                backouts: t.backouts,
                purges: t.purges,
                expunges: t.expunges,
            })
            .filter(|t| {
                t.indexed_reads
                    + t.non_indexed_reads
                    + t.inserts
                    + t.updates
                    + t.deletes
                    + t.backouts
                    + t.purges
                    + t.expunges
                    != 0
            })
            .collect();
        tables.sort_by(|a, b| a.table.cmp(&b.table));
        Self {
            reads: d.reads,
            writes: d.writes,
            fetches: d.fetches,
            marks: d.marks,
            // Memory is a level, not a counter: the latest reading
            current_memory: after.current_memory,
            max_memory: after.max_memory,
            buffers: after.num_buffers,
            page_size: after.page_size,
            tables,
        }
    }
}

/// How `execute` should go
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecuteOptions {
    /// Ask for the plans: one more round trip
    pub plan: bool,
    /// Snapshot the counters before, for `performance`: one more round trip
    /// before and one for each reading
    pub performance: bool,
}

impl Default for ExecuteOptions {
    fn default() -> Self {
        Self { plan: true, performance: true }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn col(sqltype: u32, subtype: i16, scale: i16, length: i16) -> ColumnDesc {
        ColumnDesc {
            sqltype: sqltype as i16,
            subtype,
            scale,
            length,
            nullable: true,
            field_name: String::new(),
            relation_name: String::new(),
            owner_name: String::new(),
            alias_name: "X".into(),
        }
    }

    #[test]
    fn declared_types_read_back() {
        let t = |c: ColumnDesc| ColumnInfo::describe(&c, 3).type_name;
        assert_eq!(t(col(ibase::SQL_INT64, 1, -4, 8)), "NUMERIC(18,4)");
        assert_eq!(t(col(ibase::SQL_LONG, 2, -2, 4)), "DECIMAL(9,2)");
        assert_eq!(t(col(ibase::SQL_LONG, 0, 0, 4)), "INTEGER");
        assert_eq!(t(col(ibase::SQL_VARYING, 4, 0, 240)), "VARCHAR(60)");
        assert_eq!(t(col(ibase::SQL_TEXT, 52, 0, 10)), "CHAR(10)");
        assert_eq!(t(col(ibase::SQL_BLOB, 1, 0, 8)), "BLOB SUB_TYPE TEXT");
        assert_eq!(t(col(ibase::SQL_DEC34, 0, 0, 16)), "DECFLOAT(34)");
        assert_eq!(ColumnInfo::describe(&col(ibase::SQL_TIMESTAMP, 0, 0, 8), 1).type_name, "DATE");
    }

    #[test]
    fn options_serialize_for_the_ui() {
        let o =
            TxOptions { isolation: Isolation::Snapshot, read_only: true, lock_wait: LockWait::Timeout { seconds: 5 } };
        assert_eq!(
            serde_json::to_string(&o).unwrap(),
            r#"{"isolation":"snapshot","readOnly":true,"lockWait":{"mode":"timeout","seconds":5}}"#
        );
    }
}
