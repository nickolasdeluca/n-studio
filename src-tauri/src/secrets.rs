//! Saved passwords, in the OS credential store: Keychain on macOS, the
//! Credential Manager on Windows, the Secret Service on Linux. One entry per
//! registration, keyed by its id.

use crate::error::AppResult;

const SERVICE: &str = "N Studio";

fn entry(registration_id: &str) -> AppResult<keyring::Entry> {
    Ok(keyring::Entry::new(SERVICE, registration_id)?)
}

pub fn get(registration_id: &str) -> AppResult<Option<String>> {
    match entry(registration_id)?.get_password() {
        Ok(password) => Ok(Some(password)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

pub fn set(registration_id: &str, password: &str) -> AppResult<()> {
    Ok(entry(registration_id)?.set_password(password)?)
}

pub fn delete(registration_id: &str) -> AppResult<()> {
    match entry(registration_id)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(e.into()),
    }
}
