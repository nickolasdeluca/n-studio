//! Firebird client implementation in pure rust

mod arc4;
mod blr;
mod client;
mod consts;
mod info;
mod srp;
mod util;
mod wire;
mod xsqlda;

pub use client::{
    DbHandle, RustFbClient, RustFbClientAttachmentConfig, StmtHandle, StmtHandleData, TrHandle,
};
pub use info::{DatabaseCounters, TableCounters};

#[cfg(feature = "fuzz_testing")]
pub use self::{blr::*, wire::*, xsqlda::*};
