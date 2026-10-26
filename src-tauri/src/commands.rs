//! What the UI can ask for. Every command runs on a blocking worker: they
//! wait on the network or the disk, and the UI's thread must not.
//!
//! The commands speak engine-neutral data — registrations tagged with their
//! engine, server info, the explorer tree as kinds with labels and counts —
//! so a second engine plugs in here without the UI learning about it.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use fb_core::{Connection, ServerInfo};
use fb_meta::{DbObject, ObjectKind};
use serde::Serialize;
use tauri::{AppHandle, Manager, State};

use crate::error::{AppError, AppResult};
use crate::registry::{Engine, Registration, Registry};
use crate::secrets;

pub struct AppState {
    registry: Mutex<Registry>,
    /// Open connections, by registration id
    sessions: Mutex<HashMap<String, Arc<Connection>>>,
}

impl AppState {
    pub fn new(registry: Registry) -> Self {
        Self { registry: Mutex::new(registry), sessions: Mutex::default() }
    }

    fn session(&self, id: &str) -> AppResult<Arc<Connection>> {
        self.sessions
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .ok_or_else(|| AppError::NotConnected { message: "The database is not connected.".into() })
    }
}

async fn blocking<T: Send + 'static>(
    app: AppHandle,
    f: impl FnOnce(&AppState) -> AppResult<T> + Send + 'static,
) -> AppResult<T> {
    tauri::async_runtime::spawn_blocking(move || f(&app.state::<AppState>()))
        .await
        .map_err(|e| AppError::Connection { message: format!("The worker failed: {e}") })?
}

/// A registration as the explorer lists it
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RegistrationView {
    #[serde(flatten)]
    registration: Registration,
    /// Set when connected
    server: Option<ServerInfo>,
}

/// One kind of object in the explorer tree
#[derive(Serialize)]
pub struct TreeKind {
    kind: ObjectKind,
    label: &'static str,
    count: u64,
}

#[tauri::command]
pub fn charsets() -> Vec<&'static str> {
    fb_core::charset_names().collect()
}

#[tauri::command]
pub fn list_registrations(state: State<'_, AppState>) -> Vec<RegistrationView> {
    let sessions = state.sessions.lock().unwrap();
    state
        .registry
        .lock()
        .unwrap()
        .all()
        .iter()
        .map(|r| RegistrationView {
            registration: r.clone(),
            server: sessions.get(&r.id).map(|c| c.info().clone()),
        })
        .collect()
}

/// Adds or updates a registration. `password` replaces the saved one when
/// given; a registration that stops saving its password has it deleted.
#[tauri::command]
pub async fn save_registration(
    app: AppHandle,
    registration: Registration,
    password: Option<String>,
) -> AppResult<Registration> {
    blocking(app, move |state| {
        let saved = state.registry.lock().unwrap().save(registration)?;
        match (saved.save_password, password) {
            (true, Some(password)) => secrets::set(&saved.id, &password)?,
            (true, None) => {}
            (false, _) => secrets::delete(&saved.id)?,
        }
        Ok(saved)
    })
    .await
}

#[tauri::command]
pub async fn delete_registration(app: AppHandle, id: String) -> AppResult<()> {
    blocking(app, move |state| {
        state.sessions.lock().unwrap().remove(&id);
        state.registry.lock().unwrap().remove(&id)?;
        secrets::delete(&id)
    })
    .await
}

/// Connects with the dialog's settings, reports the server, and disconnects.
/// Without a password, uses the saved one of the registration being edited.
#[tauri::command]
pub async fn test_connection(
    app: AppHandle,
    registration: Registration,
    password: Option<String>,
) -> AppResult<ServerInfo> {
    blocking(app, move |_| {
        let password = match password {
            Some(p) => p,
            None if !registration.id.is_empty() => secrets::get(&registration.id)?.unwrap_or_default(),
            None => String::new(),
        };
        let conn = open(&registration, password)?;
        Ok(conn.info().clone())
    })
    .await
}

/// Connects a registration. The password comes from the call, or else from
/// the keychain; `remember` saves a given one there.
#[tauri::command]
pub async fn connect(
    app: AppHandle,
    id: String,
    password: Option<String>,
    remember: bool,
) -> AppResult<ServerInfo> {
    blocking(app, move |state| {
        let registration = state.registry.lock().unwrap().get(&id)?.clone();
        let given = password.is_some();
        let password = match password {
            Some(p) => p,
            None if registration.save_password => match secrets::get(&id)? {
                Some(p) => p,
                None => return Err(AppError::PasswordRequired { registration: registration.name }),
            },
            None => return Err(AppError::PasswordRequired { registration: registration.name }),
        };

        let conn = open(&registration, password.clone())?;
        if given && remember {
            secrets::set(&id, &password)?;
            if !registration.save_password {
                let mut registry = state.registry.lock().unwrap();
                registry.save(Registration { save_password: true, ..registration })?;
            }
        }
        let info = conn.info().clone();
        // Replacing a session drops the old connection, which detaches.
        state.sessions.lock().unwrap().insert(id, Arc::new(conn));
        Ok(info)
    })
    .await
}

#[tauri::command]
pub fn disconnect(state: State<'_, AppState>, id: String) {
    // The connection's thread detaches once the last request using it ends.
    state.sessions.lock().unwrap().remove(&id);
}

/// The explorer's kinds for a connected database, with their counts
#[tauri::command]
pub async fn explorer_tree(app: AppHandle, id: String) -> AppResult<Vec<TreeKind>> {
    blocking(app, move |state| {
        let conn = state.session(&id)?;
        Ok(fb_meta::counts(&conn)?
            .into_iter()
            .map(|(kind, count)| TreeKind { kind, label: kind.label(), count })
            .collect())
    })
    .await
}

#[tauri::command]
pub async fn object_list(app: AppHandle, id: String, kind: ObjectKind) -> AppResult<Vec<DbObject>> {
    blocking(app, move |state| {
        let conn = state.session(&id)?;
        Ok(fb_meta::objects(&conn, kind)?)
    })
    .await
}

fn open(registration: &Registration, password: String) -> AppResult<Connection> {
    match &registration.engine {
        Engine::Firebird(settings) => Ok(Connection::open(&settings.connect_params(password))?),
    }
}
