use std::path::Path;

use serde::Serialize;

pub type AppResult<T> = Result<T, AppError>;

/// What a command reports to the UI: a `kind` to branch on, and a message
/// to show
#[derive(Debug, thiserror::Error, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AppError {
    /// The database server refused: bad SQL, login failed, a lock conflict
    #[error("{message}")]
    Server { sqlcode: i32, message: String },
    /// The server could not be reached, or the connection broke
    #[error("{message}")]
    Network { message: String },
    /// The connection is gone, or the driver gave up on a reply
    #[error("{message}")]
    Connection { message: String },
    /// The registration has no saved password; ask for one
    #[error("Enter the password for {registration}.")]
    PasswordRequired { registration: String },
    #[error("{message}")]
    NotConnected { message: String },
    #[error("{message}")]
    NotFound { message: String },
    /// A registration the dialog must fix before it can be saved
    #[error("{message}")]
    Invalid { message: String },
    /// Reading or writing the app's own files
    #[error("{message}")]
    Storage { message: String },
    #[error("{message}")]
    Keychain { message: String },
}

impl AppError {
    pub fn storage(path: &Path, e: std::io::Error) -> Self {
        AppError::Storage { message: format!("{}: {e}", path.display()) }
    }
}

impl From<fb_core::Error> for AppError {
    fn from(e: fb_core::Error) -> Self {
        match e {
            fb_core::Error::Server { sqlcode, message } => AppError::Server { sqlcode, message },
            fb_core::Error::Network { message } => AppError::Network { message },
            fb_core::Error::Client { message } => AppError::Connection { message },
            fb_core::Error::Closed => AppError::Connection { message: "The connection is closed.".into() },
        }
    }
}

impl From<keyring::Error> for AppError {
    fn from(e: keyring::Error) -> Self {
        AppError::Keychain { message: format!("Keychain: {e}") }
    }
}
