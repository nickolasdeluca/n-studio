//! Looking after a server and a database: the services manager's tools
//! (backup, restore, validation, sweep, statistics, the server's log), the
//! activity monitor, and the server's users.
//!
//! What changes something still goes the one way every change does: a form
//! when values are needed, then the script, shown before it runs. A service
//! is not a script; it runs on the server and its output comes as events.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use fb_core::{
    run_service, service_users, BackupOptions, ConnectParams, Connection, RestoreOptions, ServiceEnd, ServiceTask,
    StatisticsOptions,
};
use fb_meta::ddl::{script, Context};
use fb_meta::monitor;
use fb_meta::users::{self, User, UserChange};
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

use crate::changes::{asks, destroys, field, ActionScript, FieldKind, Form, FormField};
use crate::commands::{blocking, AppState};
use crate::error::{AppError, AppResult};
use crate::objects::{code, flag, number, text, GridColumn, GridRow, GridSection};

/// The event a running service's output comes through
pub const OUTPUT: &str = "service-output";

/// Between the parts of a row's key
const KEY_PART: char = '\u{1f}';

/// The services running, by the id the UI gave each, and how to leave them
#[derive(Default)]
pub struct Services(Mutex<HashMap<u64, Arc<AtomicBool>>>);

fn invalid<T>(message: impl Into<String>) -> AppResult<T> {
    Err(AppError::Invalid { message: message.into() })
}

fn row(cells: Vec<Option<String>>, key: Option<String>) -> GridRow {
    GridRow { cells, open: None, key }
}

fn some(value: impl ToString) -> Option<String> {
    Some(value.to_string())
}

/// A flag column's cell: there when the flag is on
fn mark(on: bool) -> Option<String> {
    on.then(|| "✓".to_string())
}

// ── The activity monitor ────────────────────────────────────────────────

fn bytes(n: i64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = n as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{n} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

fn database_section(d: &monitor::DatabaseActivity) -> GridSection {
    let gap = d.next_transaction - d.oldest_transaction;
    let pairs: Vec<(&str, String)> = vec![
        ("ODS", d.ods.clone()),
        ("Page size", d.page_size.to_string()),
        ("Pages", d.pages.to_string()),
        ("Size", bytes(d.pages * d.page_size)),
        ("Page buffers", d.page_buffers.to_string()),
        ("Dialect", d.dialect.to_string()),
        ("Created", d.created.clone().unwrap_or_default()),
        ("Oldest transaction", d.oldest_transaction.to_string()),
        ("Oldest active", d.oldest_active.to_string()),
        ("Oldest snapshot", d.oldest_snapshot.to_string()),
        ("Next transaction", d.next_transaction.to_string()),
        ("Transactions since the oldest", gap.to_string()),
        ("Sweep interval", d.sweep_interval.to_string()),
        ("Read only", if d.read_only { "yes" } else { "no" }.to_string()),
        ("Forced writes", if d.forced_writes { "yes" } else { "no" }.to_string()),
        ("Reserve space", if d.reserve_space { "yes" } else { "no" }.to_string()),
        ("Shutdown mode", d.shutdown_mode.to_string()),
        ("Backup state", d.backup_state.to_string()),
    ];
    let rows = pairs.into_iter().map(|(name, value)| row(vec![some(name), Some(value)], None)).collect();
    GridSection::new(Some("Database"), vec![text("Property"), text("Value")], rows)
}

fn attachments_section(list: &[monitor::Attachment]) -> GridSection {
    const COLUMNS: [GridColumn; 20] = [
        number("ID"),
        text("User"),
        text("Role"),
        text("State"),
        text("Address"),
        text("Host"),
        text("Process"),
        number("PID"),
        text("Client"),
        text("Connected"),
        number("Page reads"),
        number("Page writes"),
        number("Page fetches"),
        number("Page marks"),
        number("Seq. reads"),
        number("Idx. reads"),
        number("Inserts"),
        number("Updates"),
        number("Deletes"),
        number("Memory"),
    ];
    let rows = list
        .iter()
        .map(|a| {
            let state = match (a.current, a.system, a.active) {
                (true, _, _) => "this connection",
                (_, true, _) => "system",
                (_, _, true) => "active",
                _ => "idle",
            };
            let cells = vec![
                some(a.id),
                a.user.clone(),
                a.role.clone(),
                some(state),
                a.address.clone(),
                a.host.clone().or_else(|| a.os_user.clone()),
                a.process.clone(),
                a.process_id.map(|p| p.to_string()),
                a.client_version.clone(),
                a.since.clone(),
                some(a.page_reads),
                some(a.page_writes),
                some(a.page_fetches),
                some(a.page_marks),
                some(a.seq_reads),
                some(a.idx_reads),
                some(a.inserts),
                some(a.updates),
                some(a.deletes),
                some(bytes(a.memory_used)),
            ];
            // Neither this connection nor the server's own are to be killed
            row(cells, (!a.current && !a.system).then(|| a.id.to_string()))
        })
        .collect();
    let mut section = GridSection::new(Some("Attachments"), COLUMNS.to_vec(), rows);
    section.row_actions = vec![destroys("kill_attachment", "Disconnect…")];
    section
}

fn transactions_section(list: &[monitor::Transaction]) -> GridSection {
    let columns = vec![
        number("ID"),
        number("Attachment"),
        text("State"),
        text("Started"),
        text("Isolation"),
        flag("Read only"),
        flag("Auto commit"),
        text("Lock wait"),
        number("Oldest transaction"),
        number("Oldest active"),
    ];
    let rows = list
        .iter()
        .map(|t| {
            let wait = match t.lock_timeout {
                -1 => "waits".to_string(),
                0 => "no wait".to_string(),
                n => format!("{n} s"),
            };
            let cells = vec![
                some(t.id),
                some(t.attachment),
                some(if t.active { "active" } else { "idle" }),
                t.since.clone(),
                some(t.isolation),
                mark(t.read_only),
                mark(t.auto_commit),
                Some(wait),
                some(t.oldest_transaction),
                some(t.oldest_active),
            ];
            row(cells, None)
        })
        .collect();
    GridSection::new(Some("Transactions"), columns, rows)
}

fn statements_section(list: &[monitor::Statement], own: Option<i64>) -> GridSection {
    let columns =
        vec![number("ID"), number("Attachment"), number("Transaction"), text("State"), text("Started"), code("SQL")];
    let rows = list
        .iter()
        .map(|s| {
            let sql = s.sql.as_ref().map(|sql| {
                let line = sql.split_whitespace().collect::<Vec<_>>().join(" ");
                if s.cut {
                    format!("{line}…")
                } else {
                    line
                }
            });
            let cells = vec![
                some(s.id),
                some(s.attachment),
                s.transaction.map(|t| t.to_string()),
                some(s.state),
                s.since.clone(),
                sql,
            ];
            // Only what runs can be cancelled, and not from the attachment running it
            row(cells, (s.state != "idle" && Some(s.attachment) != own).then(|| s.id.to_string()))
        })
        .collect();
    let mut section = GridSection::new(Some("Statements"), columns, rows);
    section.row_actions = vec![destroys("cancel_statement", "Cancel Statement…")];
    section
}

fn monitor_sections(conn: &Connection, idle: bool) -> AppResult<Vec<GridSection>> {
    let attachments = monitor::attachments(conn)?;
    let own = attachments.iter().find(|a| a.current).map(|a| a.id);
    let mut sections = vec![attachments_section(&attachments)];
    sections.push(statements_section(&monitor::statements(conn, !idle)?, own));
    sections.push(transactions_section(&monitor::transactions(conn)?));
    if let Some(database) = monitor::database(conn)? {
        sections.push(database_section(&database));
    }
    Ok(sections)
}

// ── Users ───────────────────────────────────────────────────────────────

/// The server's users: from the security database's table where there is
/// one, through the services manager before Firebird 3
fn user_list(conn: &Connection, params: &ConnectParams) -> AppResult<Vec<User>> {
    if conn.info().major >= 3 {
        return Ok(users::users(conn)?);
    }
    let name = |s: String| Some(s).filter(|s| !s.is_empty());
    Ok(service_users(params)?
        .into_iter()
        .map(|u| User {
            name: u.name,
            first_name: name(u.first_name),
            middle_name: name(u.middle_name),
            last_name: name(u.last_name),
            active: true,
            admin: false,
            plugin: None,
            description: None,
        })
        .collect())
}

fn user_key(u: &User) -> String {
    format!("{}{KEY_PART}{}", u.name, u.plugin.as_deref().unwrap_or_default())
}

fn users_section(list: &[User], major: u32) -> GridSection {
    let later = major >= 3;
    let mut columns = vec![text("User"), text("First name"), text("Middle name"), text("Last name")];
    if later {
        columns.extend([flag("Active"), flag("Administrator"), text("Plugin"), text("Description")]);
    }
    let rows = list
        .iter()
        .map(|u| {
            let mut cells = vec![some(&u.name), u.first_name.clone(), u.middle_name.clone(), u.last_name.clone()];
            if later {
                cells.extend([mark(u.active), mark(u.admin), u.plugin.clone(), u.description.clone()]);
            }
            row(cells, Some(user_key(u)))
        })
        .collect();
    let mut section = GridSection::new(None, columns, rows);
    section.actions = vec![asks("add_user", "New User…")];
    section.row_actions = vec![asks("edit_user", "Edit…"), destroys("drop_user", "Drop…")];
    section
}

/// A view's grids: `monitor` (with the idle statements too when `idle`) or
/// `users`
#[tauri::command]
pub async fn admin_view(app: AppHandle, id: String, view: String, idle: bool) -> AppResult<Vec<GridSection>> {
    blocking(app, move |state| {
        let conn = state.session(&id)?;
        match view.as_str() {
            "monitor" => monitor_sections(&conn, idle),
            "users" => Ok(vec![users_section(&user_list(&conn, &state.params(&id)?)?, conn.info().major)]),
            _ => invalid(format!("There is no view {view}.")),
        }
    })
    .await
}

// ── Forms ───────────────────────────────────────────────────────────────

const SERVER_PATH: &str = "A path on the server, which is where the file is read or written.";
const PLUGINS: [&str; 2] = ["Srp", "Legacy_UserManager"];

fn check(id: &'static str, label: &'static str, on: bool) -> FormField {
    field(id, label, FieldKind::Check).value(if on { "true" } else { "false" })
}

/// The user a row's key names
fn keyed_user(list: Vec<User>, key: &Option<String>) -> AppResult<User> {
    let key = key.as_deref().unwrap_or_default();
    match list.into_iter().find(|u| user_key(u) == key) {
        Some(user) => Ok(user),
        None => Err(AppError::NotFound { message: "That user is no longer there.".into() }),
    }
}

fn user_fields(major: u32, user: Option<&User>) -> Vec<FormField> {
    use FieldKind::*;
    let name = |n: Option<&Option<String>>| n.and_then(|n| n.clone()).unwrap_or_default();
    let mut fields = Vec::new();
    match user {
        None => {
            fields.push(field("name", "Name", Text).required());
            fields.push(field("password", "Password", Password).required());
        }
        Some(_) => fields.push(field("password", "New password", Password).hint("Empty, the password stays as it is.")),
    }
    fields.push(field("first_name", "First name", Text).value(name(user.map(|u| &u.first_name))));
    fields.push(field("middle_name", "Middle name", Text).value(name(user.map(|u| &u.middle_name))));
    fields.push(field("last_name", "Last name", Text).value(name(user.map(|u| &u.last_name))));
    if major >= 3 {
        fields.push(check("active", "Active", user.is_none_or(|u| u.active)).hint("An inactive user cannot connect. Only the Srp plugin keeps this."));
        fields.push(
            check("admin", "Administrator", user.is_some_and(|u| u.admin))
                .hint("Holds RDB$ADMIN in the security database: may manage users."),
        );
        if user.is_none() {
            fields.push(
                field("plugin", "Plugin", Combo)
                    .options(PLUGINS)
                    .hint("The user manager that keeps the user. Empty, the server's first."),
            );
        }
    } else if user.is_none() {
        fields.push(check("admin", "Administrator", false).hint("Grants RDB$ADMIN in the security database."));
    }
    fields
}

fn form(conn: &Connection, params: &ConnectParams, tool: &str, key: &Option<String>) -> AppResult<Form> {
    use FieldKind::*;
    let (title, fields) = match tool {
        "backup" => (
            params.database.clone(),
            vec![
                field("file", "Backup file", Text).required().hint(SERVER_PATH),
                check("no_garbage_collect", "Do not collect garbage", true)
                    .hint("Faster, and what a backup made to be restored wants."),
                check("metadata_only", "Metadata only", false),
                check("ignore_checksums", "Ignore checksum errors", false),
                check("ignore_limbo", "Ignore transactions in limbo", false),
                check("convert_external", "Convert external tables", false),
                check("non_transportable", "Not transportable", false)
                    .hint("A backup only this platform reads."),
            ],
        ),
        "restore" => (
            "a backup".to_string(),
            vec![
                field("file", "Backup file", Text).required().hint(SERVER_PATH),
                field("database", "Database", Text)
                    .required()
                    .hint("A path on the server for the restored database. Not the database you are connected to."),
                check("replace", "Replace the database if it exists", false)
                    .hint("What that database holds is lost."),
                field("page_size", "Page size", Select)
                    .options(["", "4096", "8192", "16384", "32768"])
                    .hint("Empty, the size the backup was made with."),
                check("deactivate_indexes", "Leave indexes inactive", false),
                check("no_validity", "Do not check constraints", false),
                check("one_at_a_time", "Commit each table", false)
                    .hint("What can be restored is kept when a table fails."),
                check("no_shadows", "Do not restore shadows", false),
                check("use_all_space", "Fill pages completely", false)
                    .hint("For a database that will only be read."),
            ],
        ),
        "statistics" => (
            params.database.clone(),
            vec![
                check("header_only", "Header page only", true)
                    .hint("The transaction counters and the database's settings. Unchecked, what is checked below."),
                check("data_pages", "Data pages", true),
                check("index_pages", "Index pages", true),
                check("record_versions", "Records and versions", false).hint("Reads every record: slow on a large database."),
                check("system_relations", "System tables too", false),
            ],
        ),
        "add_user" => ("the server".to_string(), user_fields(conn.info().major, None)),
        "edit_user" => {
            let user = keyed_user(user_list(conn, params)?, key)?;
            (user.name.clone(), user_fields(conn.info().major, Some(&user)))
        }
        _ => return invalid(format!("{tool} asks nothing.")),
    };
    Ok(Form { title, fields })
}

/// What a tool asks before it runs. `key` is the row it acts on, if any.
#[tauri::command]
pub async fn admin_form(app: AppHandle, id: String, tool: String, key: Option<String>) -> AppResult<Form> {
    blocking(app, move |state| {
        let conn = state.session(&id)?;
        form(&conn, &state.params(&id)?, &tool, &key)
    })
    .await
}

// ── Scripts ─────────────────────────────────────────────────────────────

struct Values<'a>(&'a HashMap<String, String>);

impl Values<'_> {
    fn text(&self, id: &str) -> &str {
        self.0.get(id).map(|v| v.trim()).unwrap_or_default()
    }

    fn on(&self, id: &str) -> bool {
        self.0.get(id).is_some_and(|v| v == "true")
    }

    fn has(&self, id: &str) -> bool {
        self.0.contains_key(id)
    }
}

fn row_id(key: &Option<String>) -> AppResult<i64> {
    match key.as_deref().and_then(|k| k.parse().ok()) {
        Some(id) => Ok(id),
        None => invalid("No row was picked."),
    }
}

/// What changed between a user and a form's values
fn user_change(user: &User, v: &Values, major: u32) -> UserChange {
    let name = |id: &str, was: &Option<String>| {
        let now = v.text(id);
        (v.has(id) && now != was.as_deref().unwrap_or_default()).then(|| now.to_string())
    };
    let state = |id: &str, was: bool| (major >= 3 && v.has(id) && v.on(id) != was).then(|| v.on(id));
    UserChange {
        // A password is kept as typed: its spaces are part of it
        password: v.0.get("password").filter(|p| !p.is_empty()).cloned(),
        first_name: name("first_name", &user.first_name),
        middle_name: name("middle_name", &user.middle_name),
        last_name: name("last_name", &user.last_name),
        active: state("active", user.active),
        admin: state("admin", user.admin),
        plugin: user.plugin.clone(),
    }
}

fn action_script(
    conn: &Connection,
    params: &ConnectParams,
    tool: &str,
    key: &Option<String>,
    values: &HashMap<String, String>,
) -> AppResult<ActionScript> {
    let ctx = Context::of(conn.info());
    let v = Values(values);
    let filled = |text: &str| Some(text.to_string()).filter(|t| !t.is_empty());
    let (title, statement) = match tool {
        "kill_attachment" => {
            let id = row_id(key)?;
            (format!("attachment {id}"), monitor::kill(id, &ctx))
        }
        "cancel_statement" => {
            let id = row_id(key)?;
            (format!("statement {id}"), monitor::cancel(id))
        }
        "add_user" => {
            let name = v.text("name");
            if name.is_empty() {
                return invalid("A user needs a name.");
            }
            let change = UserChange {
                password: values.get("password").filter(|p| !p.is_empty()).cloned(),
                first_name: filled(v.text("first_name")),
                middle_name: filled(v.text("middle_name")),
                last_name: filled(v.text("last_name")),
                // Active is what a new user is: only the other is said
                active: (!v.on("active") && v.has("active")).then_some(false),
                admin: v.on("admin").then_some(true),
                plugin: filled(v.text("plugin")),
            };
            match users::create(name, &change, &ctx) {
                Some(statement) => (name.to_string(), statement),
                None => return invalid("A user needs a password."),
            }
        }
        "edit_user" => {
            let user = keyed_user(user_list(conn, params)?, key)?;
            match users::alter(&user.name, &user_change(&user, &v, ctx.major), &ctx) {
                Some(statement) => (user.name, statement),
                None => return invalid("Nothing was changed."),
            }
        }
        "drop_user" => {
            let user = keyed_user(user_list(conn, params)?, key)?;
            let statement = users::drop(&user.name, user.plugin.as_deref(), &ctx);
            (user.name, statement)
        }
        _ => return invalid(format!("{tool} is not done by a script.")),
    };
    Ok(ActionScript { script: script(&[statement]), title, renamed: None })
}

/// The script that carries a tool out, for the user to see and then to run
#[tauri::command]
pub async fn admin_script(
    app: AppHandle,
    id: String,
    tool: String,
    key: Option<String>,
    values: HashMap<String, String>,
) -> AppResult<ActionScript> {
    blocking(app, move |state| {
        let conn = state.session(&id)?;
        action_script(&conn, &state.params(&id)?, &tool, &key, &values)
    })
    .await
}

// ── Services ────────────────────────────────────────────────────────────

/// A service tool as the task it starts, on the registration's database
fn task(tool: &str, v: &Values, database: &str, major: u32) -> AppResult<ServiceTask> {
    let database = database.to_string();
    Ok(match tool {
        "backup" => ServiceTask::Backup {
            database,
            file: v.text("file").to_string(),
            options: BackupOptions {
                metadata_only: v.on("metadata_only"),
                no_garbage_collect: v.on("no_garbage_collect"),
                ignore_checksums: v.on("ignore_checksums"),
                ignore_limbo: v.on("ignore_limbo"),
                non_transportable: v.on("non_transportable"),
                convert_external: v.on("convert_external"),
            },
        },
        "restore" => {
            let target = v.text("database");
            // Restoring over the database the tool was opened from would end
            // this very session's work, by a slip of the hand
            if target.eq_ignore_ascii_case(&database) {
                return invalid("That is the database you are connected to. Restore to another path.");
            }
            ServiceTask::Restore {
                file: v.text("file").to_string(),
                database: target.to_string(),
                options: RestoreOptions {
                    replace: v.on("replace"),
                    deactivate_indexes: v.on("deactivate_indexes"),
                    no_shadows: v.on("no_shadows"),
                    no_validity: v.on("no_validity"),
                    one_at_a_time: v.on("one_at_a_time"),
                    use_all_space: v.on("use_all_space"),
                    page_size: v.text("page_size").parse().ok(),
                },
            }
        }
        "validate" => ServiceTask::Validate { database, online: major >= 3 },
        "sweep" => ServiceTask::Sweep { database },
        "statistics" => ServiceTask::Statistics {
            database,
            options: StatisticsOptions {
                header_only: v.on("header_only"),
                data_pages: v.on("data_pages"),
                index_pages: v.on("index_pages"),
                record_versions: v.on("record_versions"),
                system_relations: v.on("system_relations"),
            },
        },
        "server_log" => ServiceTask::ServerLog,
        _ => return invalid(format!("{tool} is not a service.")),
    })
}

#[derive(Debug, Clone, Serialize)]
pub struct Output {
    run: u64,
    lines: Vec<String>,
}

/// How a service ended, with the lines no [`OUTPUT`] event carried: only
/// the answer is sure to be the last thing the UI hears
#[derive(Debug, Clone, Serialize)]
pub struct ServiceResult {
    end: ServiceEnd,
    lines: Vec<String>,
}

/// Runs a service tool on a connected registration's server, with a form's
/// values when the tool has a form. `run` names it for [`service_cancel`]
/// and in its [`OUTPUT`] events.
#[tauri::command]
pub async fn service_run(
    app: AppHandle,
    id: String,
    run: u64,
    tool: String,
    values: HashMap<String, String>,
) -> AppResult<ServiceResult> {
    let events = app.clone();
    blocking(app, move |state| {
        let params = state.params(&id)?;
        let major = state.session(&id)?.info().major;
        let task = task(&tool, &Values(&values), &params.database, major)?;
        let cancel = Arc::new(AtomicBool::new(false));
        state.services.0.lock().unwrap().insert(run, cancel.clone());

        let mut pending = Output { run, lines: Vec::new() };
        let mut sent = Instant::now();
        let result = run_service(&params, &task, &cancel, |line| {
            pending.lines.push(line.to_string());
            if sent.elapsed() >= Duration::from_millis(100) || pending.lines.len() >= 500 {
                let _ = events.emit(OUTPUT, &pending);
                pending.lines.clear();
                sent = Instant::now();
            }
        });
        state.services.0.lock().unwrap().remove(&run);
        // What it wrote before failing is part of why it failed
        if result.is_err() && !pending.lines.is_empty() {
            let _ = events.emit(OUTPUT, &pending);
        }
        Ok(ServiceResult { end: result?, lines: pending.lines })
    })
    .await
}

/// Stops listening to a running service: the server is told by the
/// connection closing. `false` when no such service runs.
#[tauri::command]
pub fn service_cancel(state: State<'_, AppState>, run: u64) -> bool {
    match state.services.0.lock().unwrap().get(&run) {
        Some(cancel) => {
            cancel.store(true, Ordering::Relaxed);
            true
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    fn user() -> User {
        User {
            name: "ANA".into(),
            first_name: Some("Ana".into()),
            middle_name: None,
            last_name: None,
            active: true,
            admin: false,
            plugin: Some("Srp".into()),
            description: None,
        }
    }

    #[test]
    fn a_restore_never_lands_on_the_connected_database() {
        let v = values(&[("file", "/b/x.fbk"), ("database", "/Data/Main.fdb"), ("replace", "true")]);
        assert!(task("restore", &Values(&v), "/data/main.fdb", 4).is_err());
        let v = values(&[("file", "/b/x.fbk"), ("database", "/data/copy.fdb"), ("page_size", "8192")]);
        let ServiceTask::Restore { database, options, .. } = task("restore", &Values(&v), "/data/main.fdb", 4).unwrap()
        else {
            panic!("a restore")
        };
        assert_eq!(database, "/data/copy.fdb");
        assert_eq!((options.replace, options.page_size), (false, Some(8192)));
    }

    #[test]
    fn validation_is_online_where_the_server_has_it() {
        let v = values(&[]);
        let online = |major| matches!(task("validate", &Values(&v), "db", major), Ok(ServiceTask::Validate { online: true, .. }));
        assert!(online(3) && online(5) && !online(2));
    }

    #[test]
    fn editing_a_user_says_only_what_changed() {
        let same = values(&[("password", ""), ("first_name", "Ana"), ("middle_name", ""), ("active", "true"), ("admin", "false")]);
        let change = user_change(&user(), &Values(&same), 4);
        assert_eq!(change, UserChange { plugin: Some("Srp".into()), ..Default::default() });

        let other = values(&[("password", " p w "), ("first_name", ""), ("active", "false"), ("admin", "true")]);
        let change = user_change(&user(), &Values(&other), 4);
        assert_eq!(change.password.as_deref(), Some(" p w "));
        assert_eq!(change.first_name.as_deref(), Some(""));
        assert_eq!((change.active, change.admin), (Some(false), Some(true)));
        // Firebird 2.5 keeps no state to change
        assert_eq!(user_change(&user(), &Values(&other), 2).active, None);
    }

    #[test]
    fn rows_that_cannot_be_acted_on_have_no_key() {
        let attachment = |id, current, system| monitor::Attachment {
            id,
            current,
            system,
            active: true,
            user: None,
            role: None,
            protocol: None,
            address: None,
            host: None,
            os_user: None,
            process: None,
            process_id: None,
            client_version: None,
            server_pid: None,
            since: None,
            page_reads: 0,
            page_writes: 0,
            page_fetches: 0,
            page_marks: 0,
            seq_reads: 0,
            idx_reads: 0,
            inserts: 0,
            updates: 0,
            deletes: 0,
            memory_used: 2048,
        };
        let section = attachments_section(&[attachment(1, true, false), attachment(2, false, true), attachment(3, false, false)]);
        let keys: Vec<_> = section.rows.iter().map(|r| r.key.clone()).collect();
        assert_eq!(keys, [None, None, Some("3".to_string())]);
        assert_eq!(GridSection::cell(&section.rows[2], 19).as_deref(), Some("2.0 KB"));
    }
}
