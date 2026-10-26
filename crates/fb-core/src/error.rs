use rsfbclient_core::FbError;
use serde::Serialize;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, Clone, PartialEq, thiserror::Error, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Error {
    /// The server refused: bad SQL, wrong password, a lock conflict
    #[error("{message}")]
    Server { sqlcode: i32, message: String },

    /// The server could not be reached, or the connection broke
    #[error("{message}")]
    Network { message: String },

    /// The client gave up: a malformed reply, a value it cannot decode
    #[error("{message}")]
    Client { message: String },

    /// The connection was closed, or its thread is gone
    #[error("the connection is closed")]
    Closed,
}

impl From<FbError> for Error {
    fn from(e: FbError) -> Self {
        match e {
            FbError::Sql { msg, code } => Error::Server { sqlcode: code, message: msg },
            FbError::Io(e) => Error::Network { message: e.to_string() },
            FbError::Other(message) => Error::Client { message },
        }
    }
}
