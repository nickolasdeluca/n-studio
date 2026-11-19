//! What the UI can ask for. Every command runs on a blocking worker: they
//! wait on the network or the disk, and the UI's thread must not.
//!
//! The commands speak engine-neutral data — registrations tagged with their
//! engine, server info, the explorer tree as kinds with labels and counts —
//! so a second engine plugs in here without the UI learning about it.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use fb_core::{ConnectParams, Connection, ServerInfo};
use fb_meta::{DbObject, ObjectKind};
use serde::Serialize;
use tauri::{AppHandle, Manager, State};

use crate::admin::Services;
use crate::editor::Results;
use crate::error::{AppError, AppResult};
use crate::registry::{Engine, Registration, Registry};
use crate::executive::Executives;
use crate::script::Scripts;
use crate::history::History;
use crate::secrets;
use crate::session::SessionStore;
use crate::formatter::StyleStore;
use crate::debugger::Debuggers;
use crate::tools::Comparisons;
use crate::trace::Traces;

pub struct AppState {
    pub(crate) registry: Mutex<Registry>,
    /// Open connections, by registration id
    sessions: Mutex<HashMap<String, Arc<Connection>>>,
    pub(crate) results: Results,
    pub(crate) scripts: Scripts,
    pub(crate) executives: Executives,
    pub(crate) services: Services,
    pub(crate) comparisons: Comparisons,
    pub(crate) debuggers: Debuggers,
    pub(crate) traces: Traces,
    /// What each open connection was made with, for the services manager:
    /// a connection of its own, to the same server as the same user. Kept
    /// only while connected, and only in memory.
    params: Mutex<HashMap<String, ConnectParams>>,
    /// The connection the activity monitor's grids read through, by
    /// registration, with the attachment id of the registration's own: reading
    /// the monitoring tables is a query, and a slow one on a busy server, that
    /// should neither wait behind the user's work nor hold it up
    monitors: Mutex<HashMap<String, (Arc<Connection>, i64)>>,
    pub(crate) session_store: SessionStore,
    pub(crate) history: History,
    pub(crate) style_store: StyleStore,
}

impl AppState {
    pub fn new(
        registry: Registry,
        session_store: SessionStore,
        history: History,
        style_store: StyleStore,
        traces: Traces,
    ) -> Self {
        Self {
            registry: Mutex::new(registry),
            sessions: Mutex::default(),
            results: Results::default(),
            scripts: Scripts::default(),
            executives: Executives::default(),
            services: Services::default(),
            comparisons: Comparisons::default(),
            debuggers: Debuggers::default(),
            traces,
            params: Mutex::default(),
            monitors: Mutex::default(),
            session_store,
            history,
            style_store,
        }
    }

    pub(crate) fn session(&self, id: &str) -> AppResult<Arc<Connection>> {
        self.sessions
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .ok_or_else(|| AppError::NotConnected { message: "The database is not connected.".into() })
    }

    /// The connected registrations, in the explorer's order, each with its
    /// connection
    pub(crate) fn connected(&self) -> Vec<(Registration, Arc<Connection>)> {
        self.registered().into_iter().filter_map(|(r, conn)| conn.map(|conn| (r, conn))).collect()
    }

    /// Every registration, in the explorer's order, with its connection
    /// when it is connected
    pub(crate) fn registered(&self) -> Vec<(Registration, Option<Arc<Connection>>)> {
        let sessions = self.sessions.lock().unwrap();
        let registry = self.registry.lock().unwrap();
        registry.all().iter().map(|r| (r.clone(), sessions.get(&r.id).cloned())).collect()
    }

    /// The connection the monitor reads through, opened the first time it is
    /// asked for, and the attachment id of the registration's own connection
    pub(crate) fn monitor(&self, id: &str) -> AppResult<(Arc<Connection>, i64)> {
        if let Some(found) = self.monitors.lock().unwrap().get(id) {
            return Ok(found.clone());
        }
        let main = self.session(id)?;
        let own = main.query("select current_connection from rdb$database", vec![])?.rows[0][0].as_i64().unwrap_or(0);
        let monitor = (Arc::new(Connection::open(&self.params(id)?)?), own);
        // Asked twice at once, the first one opened is the one kept
        Ok(self.monitors.lock().unwrap().entry(id.to_string()).or_insert(monitor).clone())
    }

    /// The monitor's connection is done with, or failed: the next read opens another
    pub(crate) fn forget_monitor(&self, id: &str) {
        self.monitors.lock().unwrap().remove(id);
    }

    /// How a connected registration reaches its server
    pub(crate) fn params(&self, id: &str) -> AppResult<ConnectParams> {
        self.params
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .ok_or_else(|| AppError::NotConnected { message: "The database is not connected.".into() })
    }

    fn forget(&self, id: &str) {
        self.sessions.lock().unwrap().remove(id);
        self.params.lock().unwrap().remove(id);
        self.monitors.lock().unwrap().remove(id);
        self.results.close_session(id);
    }
}

pub(crate) async fn blocking<T: Send + 'static>(
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
    /// One of the kind: "Table"
    singular: &'static str,
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
    password_of: Option<String>,
) -> AppResult<Registration> {
    blocking(app, move |state| {
        // A clone keeps its source's password unless another is typed
        let password = match (password, &password_of) {
            (None, Some(source)) => known_password(state, source)?,
            (password, _) => password,
        };
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

/// The password a registration connects with: its open session's, else
/// the keychain's when it saves one
fn known_password(state: &AppState, id: &str) -> AppResult<Option<String>> {
    if let Ok(params) = state.params(id) {
        return Ok(Some(params.password));
    }
    let saves = state.registry.lock().unwrap().get(id).is_ok_and(|r| r.save_password);
    if saves {
        secrets::get(id)
    } else {
        Ok(None)
    }
}

/// Puts a registration in a folder, before or after another one, or last
#[tauri::command]
pub async fn place_registration(
    app: AppHandle,
    id: String,
    folder: Vec<String>,
    anchor: Option<String>,
    after: bool,
) -> AppResult<()> {
    blocking(app, move |state| state.registry.lock().unwrap().place(&id, &folder, anchor.as_deref(), after)).await
}

/// Renames or moves an explorer folder, with every registration under it
#[tauri::command]
pub async fn move_folder(app: AppHandle, from: Vec<String>, to: Vec<String>) -> AppResult<usize> {
    blocking(app, move |state| state.registry.lock().unwrap().move_folder(&from, &to)).await
}

#[tauri::command]
pub async fn delete_registration(app: AppHandle, id: String) -> AppResult<()> {
    blocking(app, move |state| {
        state.forget(&id);
        state.registry.lock().unwrap().remove(&id)?;
        // What it ran goes with it
        state.history.clear(&id)?;
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
    password_of: Option<String>,
) -> AppResult<ServerInfo> {
    blocking(app, move |state| {
        let password = match (password, &password_of) {
            (Some(p), _) => p,
            (None, Some(source)) => known_password(state, source)?.unwrap_or_default(),
            (None, None) if !registration.id.is_empty() => secrets::get(&registration.id)?.unwrap_or_default(),
            (None, None) => String::new(),
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

        let Engine::Firebird(settings) = &registration.engine;
        let params = settings.connect_params(password.clone());
        let conn = Connection::open(&params)?;
        if given && remember {
            secrets::set(&id, &password)?;
            if !registration.save_password {
                let mut registry = state.registry.lock().unwrap();
                registry.save(Registration { save_password: true, ..registration })?;
            }
        }
        let info = conn.info().clone();
        // Replacing a session drops the old connection, which detaches.
        state.results.close_session(&id);
        state.forget_monitor(&id);
        state.params.lock().unwrap().insert(id.clone(), params);
        state.sessions.lock().unwrap().insert(id, Arc::new(conn));
        Ok(info)
    })
    .await
}

#[tauri::command]
pub fn disconnect(state: State<'_, AppState>, id: String) {
    // The connection's thread detaches once the last request using it ends.
    state.forget(&id);
}

/// The explorer's kinds for a connected database, with their counts
#[tauri::command]
pub async fn explorer_tree(app: AppHandle, id: String) -> AppResult<Vec<TreeKind>> {
    blocking(app, move |state| {
        let conn = state.session(&id)?;
        Ok(fb_meta::counts(&conn)?
            .into_iter()
            .map(|(kind, count)| TreeKind { kind, label: kind.label(), singular: kind.singular(), count })
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

/// Every name the SQL editor completes on this registration
#[tauri::command]
pub async fn completion_catalog(app: AppHandle, id: String) -> AppResult<fb_meta::complete::Catalog> {
    blocking(app, move |state| {
        let conn = state.session(&id)?;
        Ok(fb_meta::complete::catalog(&conn)?)
    })
    .await
}

pub(crate) fn open(registration: &Registration, password: String) -> AppResult<Connection> {
    match &registration.engine {
        Engine::Firebird(settings) => Ok(Connection::open(&settings.connect_params(password))?),
    }
}
