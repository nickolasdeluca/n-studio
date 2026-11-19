//! Connections to Firebird, over the vendored pure-Rust wire driver.
//!
//! Each [`Connection`] owns one OS thread that holds the attachment and runs
//! every request in order: a Firebird attachment is not safe to drive from
//! several threads, and this way nothing above here has to care. Calls block
//! until the connection's thread answers.

mod connection;
mod editor;
mod error;
mod executive;
mod params;
mod script;
mod service;
mod trace;
mod value;

pub use connection::{Connection, QueryResult, ServerInfo};
pub use editor::{
    BlobContent, ColumnInfo, ColumnKind, CursorId, ExecuteOptions, Executed, Fetched, Isolation, LockWait,
    Performance, Records, StatementType, TablePerformance, TxEnd, TxId, TxOptions,
};
pub use error::{Error, Result};
pub use executive::{
    run_executive, Connector, RunOptions, RunOutcome, RunStep, RunSummary, Target, Within, DEEPEST_INPUT, NO_CONNECTION,
};
pub use script::{run_script, ScriptOptions, ScriptSummary, Step, StepOutcome};
pub use service::{
    run_service, server_version, service_users, BackupOptions, OnlineMode, Property, RestoreOptions, ServiceEnd,
    ServiceTask, ServiceUser, ShutdownMode, ShutdownWait, StatisticsOptions,
};
pub use trace::{
    run_trace, trace_config, trace_database, TableCounts, TraceEnd, TraceEvent, TraceItem, TraceOptions, TraceOutcome,
    TraceReader,
};
pub use params::{charset_names, decode_as, split_database, AuthPlugin, ConnectParams, Dialect};
pub use rsfbclient_core::Decimal;
pub use value::Value;
