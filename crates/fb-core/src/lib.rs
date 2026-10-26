//! Connections to Firebird, over the vendored pure-Rust wire driver.
//!
//! Each [`Connection`] owns one OS thread that holds the attachment and runs
//! every request in order: a Firebird attachment is not safe to drive from
//! several threads, and this way nothing above here has to care. Calls block
//! until the connection's thread answers.

mod connection;
mod editor;
mod error;
mod params;
mod value;

pub use connection::{Connection, QueryResult, ServerInfo};
pub use editor::{
    BlobContent, ColumnInfo, ColumnKind, CursorId, ExecuteOptions, Executed, Fetched, Isolation, LockWait,
    Performance, Records, StatementType, TablePerformance, TxEnd, TxId, TxOptions,
};
pub use error::{Error, Result};
pub use params::{charset_names, AuthPlugin, ConnectParams, Dialect};
pub use rsfbclient_core::Decimal;
pub use value::Value;
