//! The agent's tools. They read metadata, never data: what holds a value
//! (a generator's current one) is left out, a query's rows go to the
//! user's grid and not here, and an error a query failed with has the
//! values it quotes taken out. Rows reach the agent only as the user
//! shares them.

use std::sync::Arc;
use std::time::Duration;

use fb_core::Connection;
use fb_meta::ObjectKind;
use fb_sql::{Operation, StatementKind};
use serde_json::{json, Map, Value};
use tauri::{AppHandle, Manager};

use super::{Agents, Request};
use crate::commands::AppState;
use crate::registry::Registration;

/// The tools that only read metadata, the server's activity or what was
/// shared: the agent runs them without asking the user each time
pub const READ_ONLY: [&str; 8] = [
    "list_databases",
    "list_objects",
    "get_object_ddl",
    "get_dependencies",
    "find_columns",
    "get_activity",
    "get_trace_summary",
    "get_shared_data",
];

const ALL: [&str; 13] = [
    "list_databases",
    "connect_database",
    "disconnect_database",
    "list_objects",
    "get_object_ddl",
    "get_dependencies",
    "find_columns",
    "get_activity",
    "get_trace_summary",
    "run_query",
    "open_in_editor",
    "count_affected_rows",
    "get_shared_data",
];

/// The most names `list_objects` gives, and columns `find_columns` does
const MAX_OBJECTS: usize = 2_000;
const MAX_COLUMNS: usize = 300;

/// How long a query may take, the user's fetching included, and how long
/// the window may take to open an editor
pub(super) const RUN_WAIT: Duration = Duration::from_secs(60 * 60);
const OPEN_WAIT: Duration = Duration::from_secs(60);
/// How long the user may take over a connection's password, and the window
/// over a disconnection
const CONNECT_WAIT: Duration = Duration::from_secs(10 * 60);
const DISCONNECT_WAIT: Duration = Duration::from_secs(60);

pub fn exists(name: &str) -> bool {
    ALL.contains(&name)
}

const KINDS: [&str; 12] = [
    "domain",
    "table",
    "view",
    "procedure",
    "function",
    "package",
    "trigger",
    "generator",
    "exception",
    "udf",
    "role",
    "index",
];

pub fn definitions() -> Vec<Value> {
    let database = json!({
        "type": "string",
        "description": "The database: its id or its name, as list_databases gives them. It must be connected (connect_database). May be left out when only one is.",
    });
    let kind = json!({ "type": "string", "enum": KINDS, "description": "The kind of object" });
    vec![
        json!({
            "name": "list_databases",
            "description": "The databases registered in N Studio, connected or not, with the explorer folder each is in; a connected one also with its server version, SQL dialect and default character set. The other tools work on connected ones: connect_database connects one.",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false },
        }),
        json!({
            "name": "connect_database",
            "description": "Asks N Studio to connect a registered database, as the user would from the explorer. Its saved password is used; without one the user is asked for it in N Studio (you never see it), and may decline.",
            "inputSchema": {
                "type": "object",
                "properties": { "database": { "type": "string", "description": "Its id or its name, as list_databases gives them" } },
                "required": ["database"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "disconnect_database",
            "description": "Asks N Studio to disconnect a database. Refused while the user has a transaction open on it, a statement running, or changes not applied: the user ends those first. Its editors close and come back when it connects again.",
            "inputSchema": {
                "type": "object",
                "properties": { "database": { "type": "string", "description": "Its id or its name, as list_databases gives them" } },
                "required": ["database"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "list_objects",
            "description": "The names of a database's objects of one kind (user objects, not system ones), with the table a trigger or index belongs to and the start of each description.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "database": database,
                    "kind": kind,
                    "filter": { "type": "string", "description": "Only names containing this, in any case" },
                },
                "required": ["kind"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "get_object_ddl",
            "description": "The DDL that creates an object again, as N Studio extracts it: a table with its columns, keys, checks, indices, triggers, comments and grants; a procedure, function, trigger or view with its source. No data.",
            "inputSchema": {
                "type": "object",
                "properties": { "database": database, "kind": kind, "name": { "type": "string" } },
                "required": ["kind", "name"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "get_dependencies",
            "description": "What an object uses, and what uses it: the impact of changing or dropping it.",
            "inputSchema": {
                "type": "object",
                "properties": { "database": database, "kind": kind, "name": { "type": "string" } },
                "required": ["kind", "name"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "find_columns",
            "description": "Columns of tables and views whose name, or whose table's name, contains a text, with type, nullability, default and description. For finding where a field is, or what a field is called.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "database": database,
                    "pattern": { "type": "string", "description": "Text in the column's or the table's name, in any case" },
                },
                "required": ["pattern"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "get_activity",
            "description": "What a connected database is doing now, from its MON$ tables: the transaction counters (oldest, oldest active, oldest snapshot, next) and the gaps between them, header settings, attachments counted and the busiest by I/O, the oldest active transactions, the statements running (literals replaced by ?) and memory. Who is attached (user, machine, program) is left out. Only SYSDBA or the database's owner sees every attachment.",
            "inputSchema": {
                "type": "object",
                "properties": { "database": database },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "get_trace_summary",
            "description": "What the newest Activity Monitor trace session of a database adds up to (one the user is recording, or opened from a log): events, errors, average and worst times over the seconds kept, and the statements that cost the most (literals replaced by ?), with runs, times, fetches, reads, records read without an index, plan and memory. The user starts and stops a session in the Activity Monitor; it is not started from here.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "database": database,
                    "by": {
                        "type": "string",
                        "enum": ["time", "runs", "slowest", "fetches", "natural", "records", "memory"],
                        "description": "What the statements are ranked by: total time unless said",
                    },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 50, "description": "How many statements: 15 unless said" },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "run_query",
            "description": "Runs one SELECT (or WITH … SELECT) in a new page of an N Studio editor, in a read-only transaction. The user sees the rows; you do not. You are told whether it ran, the columns and their types, how many rows were fetched, the times and the plan, or the error (with the values it quoted removed). Anything else than one SELECT is refused: use open_in_editor for it.",
            "inputSchema": {
                "type": "object",
                "properties": { "database": database, "sql": { "type": "string" } },
                "required": ["sql"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "open_in_editor",
            "description": "Opens SQL in a new N Studio editor for the user to read, change and run: DDL, a migration script, DML. It is not run. This is how a database is changed. A script of many statements, one that creates or alters procedures, triggers or functions with SET TERM, or one with its own COMMITs, goes to a Script Executive (in: script_executive), which runs a script statement by statement as isql does; a statement or a few go to an SQL editor (in: editor, the default).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "database": database,
                    "sql": { "type": "string" },
                    "title": { "type": "string", "description": "What the editor's page is called (an SQL editor only)" },
                    "in": { "type": "string", "enum": ["editor", "script_executive"], "description": "Where it opens: editor unless said" },
                },
                "required": ["sql"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "count_affected_rows",
            "description": "How many rows an UPDATE or DELETE would change, without running it: N Studio counts the rows its WHERE (with its PLAN, ORDER BY and ROWS) picks, in a read-only transaction, and gives the number only. Use it to check a change before opening it for the user. Triggers and cascades are not followed; the count is of the statement's own table.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "database": database,
                    "sql": { "type": "string", "description": "One UPDATE or DELETE, without parameters" },
                },
                "required": ["sql"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "get_shared_data",
            "description": "Rows the user chose to share from a grid. Without an id, the list of what was shared; with one, its columns and rows.",
            "inputSchema": {
                "type": "object",
                "properties": { "id": { "type": "string" } },
                "additionalProperties": false,
            },
        }),
    ]
}

pub fn call(app: &AppHandle, name: &str, args: &Value) -> Result<String, String> {
    let state = app.state::<AppState>();
    let value = match name {
        "list_databases" => list_databases(&state),
        "connect_database" => connect_database(app, &state, args),
        "disconnect_database" => disconnect_database(app, &state, args),
        "list_objects" => list_objects(&state, args),
        "get_object_ddl" => object_ddl(&state, args).map(Value::String),
        "get_dependencies" => dependencies(&state, args),
        "find_columns" => find_columns(&state, args),
        "run_query" => run_query(app, &state, args),
        "open_in_editor" => open_in_editor(app, &state, args),
        "count_affected_rows" => count_affected(&state, args),
        "get_activity" => activity(&state, args),
        "get_trace_summary" => trace_summary(&state, args),
        "get_shared_data" => shared_data(&app.state::<Agents>(), args),
        _ => Err(format!("There is no tool {name}.")),
    }?;
    Ok(match value {
        Value::String(text) => text,
        other => serde_json::to_string_pretty(&other).unwrap_or_default(),
    })
}

fn text<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty())
}

fn required<'a>(args: &'a Value, key: &str) -> Result<&'a str, String> {
    text(args, key).ok_or_else(|| format!("`{key}` is required."))
}

/// The registered database `wanted` names, by id, else by name in any case
fn find<'a, T>(list: &'a [(Registration, T)], wanted: &str) -> Option<&'a (Registration, T)> {
    list.iter().find(|(r, _)| r.id == wanted).or_else(|| list.iter().find(|(r, _)| r.name.eq_ignore_ascii_case(wanted)))
}

/// The registered database the arguments name, connected or not
fn registration(state: &AppState, args: &Value) -> Result<(Registration, Option<Arc<Connection>>), String> {
    let wanted = required(args, "database")?;
    find(&state.registered(), wanted)
        .cloned()
        .ok_or_else(|| format!("No database is registered as {wanted}; list_databases says which are."))
}

/// The connected database the arguments name, or the only one connected
fn database(state: &AppState, args: &Value) -> Result<(Registration, Arc<Connection>), String> {
    match text(args, "database") {
        Some(wanted) => match find(&state.registered(), wanted) {
            Some((r, Some(conn))) => Ok((r.clone(), conn.clone())),
            Some((r, None)) => Err(format!("{} is not connected; connect_database connects it.", r.name)),
            None => Err(format!("No database is registered as {wanted}; list_databases says which are.")),
        },
        None => match state.connected().as_slice() {
            [one] => Ok(one.clone()),
            [] => Err("No database is connected in N Studio: connect_database connects one.".into()),
            _ => Err("More than one database is connected: say which with `database`.".into()),
        },
    }
}

fn kind(args: &Value) -> Result<ObjectKind, String> {
    let kind = required(args, "kind")?;
    serde_json::from_value(Value::String(kind.to_ascii_lowercase()))
        .map_err(|_| format!("`kind` is one of {}.", KINDS.join(", ")))
}

/// The name as the catalog has it: as given, else the one that differs
/// from it in case only
fn object_name(conn: &Connection, kind: ObjectKind, name: &str) -> Result<String, String> {
    let objects = fb_meta::objects(conn, kind).map_err(|e| e.to_string())?;
    let exact = objects.iter().find(|o| o.name == name);
    let any_case = || objects.iter().find(|o| o.name.eq_ignore_ascii_case(name));
    exact
        .or_else(any_case)
        .map(|o| o.name.clone())
        .ok_or_else(|| format!("There is no {} {name}.", kind.singular().to_lowercase()))
}

/// A registration as the agent is told of it: no server, path or user,
/// which it has no need of and which would leave the machine with it
fn described(r: &Registration, conn: Option<&Arc<Connection>>) -> Value {
    let mut entry = Map::new();
    entry.insert("database".into(), json!(r.id));
    entry.insert("name".into(), json!(r.name));
    if !r.folder.is_empty() {
        entry.insert("folder".into(), json!(r.folder.join(" / ")));
    }
    entry.insert("connected".into(), json!(conn.is_some()));
    if let Some(conn) = conn {
        let info = conn.info();
        entry.insert("server".into(), json!(format!("Firebird {}", info.engine_version)));
        entry.insert("dialect".into(), json!(info.dialect));
        entry.insert("charset".into(), json!(info.charset));
        entry.insert("ods".into(), json!(format!("{}.{}", info.ods_major, info.ods_minor)));
    }
    Value::Object(entry)
}

fn list_databases(state: &AppState) -> Result<Value, String> {
    let registered = state.registered();
    if registered.is_empty() {
        return Err("No database is registered in N Studio. Ask the user to register one.".into());
    }
    Ok(Value::Array(registered.iter().map(|(r, conn)| described(r, conn.as_ref())).collect()))
}

/// What the window said went wrong, if it did
fn refusal(reply: &Value) -> Option<String> {
    let ok = reply.get("ok").and_then(Value::as_bool).unwrap_or(false);
    (!ok).then(|| reply.get("error").and_then(Value::as_str).unwrap_or("N Studio did not say why.").to_string())
}

fn connect_database(app: &AppHandle, state: &AppState, args: &Value) -> Result<Value, String> {
    let (registration, conn) = registration(state, args)?;
    if let Some(conn) = conn {
        return Ok(json!({ "alreadyConnected": true, "database": described(&registration, Some(&conn)) }));
    }
    let agents = app.state::<Agents>();
    let reply =
        agents.ask(app, |request| Request::Connect { request, registration: registration.id.clone() }, CONNECT_WAIT)?;
    if let Some(error) = refusal(&reply) {
        return Err(error);
    }
    let conn = state.session(&registration.id).map_err(|e| e.to_string())?;
    Ok(json!({ "connected": true, "database": described(&registration, Some(&conn)) }))
}

fn disconnect_database(app: &AppHandle, state: &AppState, args: &Value) -> Result<Value, String> {
    let (registration, conn) = registration(state, args)?;
    if conn.is_none() {
        return Ok(json!(format!("{} is not connected.", registration.name)));
    }
    let agents = app.state::<Agents>();
    let reply = agents.ask(
        app,
        |request| Request::Disconnect { request, registration: registration.id.clone() },
        DISCONNECT_WAIT,
    )?;
    match refusal(&reply) {
        Some(error) => Err(error),
        None => Ok(json!(format!("{} was disconnected.", registration.name))),
    }
}

fn list_objects(state: &AppState, args: &Value) -> Result<Value, String> {
    let (_, conn) = database(state, args)?;
    let kind = kind(args)?;
    let filter = text(args, "filter").map(str::to_lowercase);
    let objects = fb_meta::objects(&conn, kind).map_err(|e| e.to_string())?;
    let matching: Vec<_> =
        objects.iter().filter(|o| filter.as_ref().is_none_or(|f| o.name.to_lowercase().contains(f))).collect();
    let listed: Vec<Value> = matching
        .iter()
        .take(MAX_OBJECTS)
        .map(|o| {
            let mut entry = Map::new();
            entry.insert("name".into(), json!(o.name));
            if let Some(parent) = &o.parent {
                entry.insert("table".into(), json!(parent));
            }
            if let Some(description) = &o.description {
                entry.insert("description".into(), json!(description));
            }
            Value::Object(entry)
        })
        .collect();
    Ok(json!({ "kind": kind, "total": matching.len(), "listed": listed.len(), "objects": listed }))
}

fn object_ddl(state: &AppState, args: &Value) -> Result<String, String> {
    let (_, conn) = database(state, args)?;
    let kind = kind(args)?;
    let name = object_name(&conn, kind, required(args, "name")?)?;
    if kind == ObjectKind::Generator {
        // Its value is the last number it gave: data, not its shape
        let mut generator = fb_meta::generator(&conn, &name).map_err(|e| e.to_string())?;
        generator.value = None;
        let ctx = fb_meta::ddl::Context::of(conn.info());
        return Ok(fb_meta::ddl::script(&fb_meta::ddl::generator(&generator, &ctx)));
    }
    fb_meta::extract::script(&conn, kind, &name).map_err(|e| e.to_string())
}

fn dependencies(state: &AppState, args: &Value) -> Result<Value, String> {
    let (_, conn) = database(state, args)?;
    let kind = kind(args)?;
    let name = object_name(&conn, kind, required(args, "name")?)?;
    let deps = fb_meta::dependencies(&conn, kind, &name).map_err(|e| e.to_string())?;
    serde_json::to_value(deps).map_err(|e| e.to_string())
}

fn find_columns(state: &AppState, args: &Value) -> Result<Value, String> {
    let (_, conn) = database(state, args)?;
    let pattern = required(args, "pattern")?.to_lowercase();
    let info = conn.info();
    let columns = fb_meta::columns(&conn, None).map_err(|e| e.to_string())?;
    let matching: Vec<_> = columns
        .iter()
        .filter(|(table, c)| table.to_lowercase().contains(&pattern) || c.name.to_lowercase().contains(&pattern))
        .collect();
    let listed: Vec<Value> = matching
        .iter()
        .take(MAX_COLUMNS)
        .map(|(table, c)| {
            let mut entry = Map::new();
            entry.insert("table".into(), json!(table));
            entry.insert("column".into(), json!(c.name));
            entry.insert("type".into(), json!(c.data_type.declaration(info.dialect, &info.charset)));
            entry.insert("notNull".into(), json!(c.required()));
            if let Some(domain) = &c.domain {
                entry.insert("domain".into(), json!(domain));
            }
            if let Some(default) = c.effective_default() {
                entry.insert("default".into(), json!(default));
            }
            if let Some(computed) = &c.computed {
                entry.insert("computedBy".into(), json!(computed));
            }
            if c.identity.is_some() {
                entry.insert("identity".into(), json!(true));
            }
            if let Some(description) = &c.description {
                entry.insert("description".into(), json!(description));
            }
            Value::Object(entry)
        })
        .collect();
    Ok(json!({ "total": matching.len(), "listed": listed.len(), "columns": listed }))
}

/// The one select `sql` is, without its terminator; anything else is refused
pub(super) fn one_select(sql: &str) -> Result<String, String> {
    const ONLY: &str = "run_query runs one SELECT and nothing else; use open_in_editor for the rest.";
    let statements = fb_sql::split(sql);
    if statements.iter().any(|s| s.kind != StatementKind::Sql) {
        return Err(ONLY.into());
    }
    let [statement] = statements.as_slice() else {
        return Err(ONLY.into());
    };
    if statement.unterminated_literal {
        return Err("The query ends inside a string, a quoted name or a comment.".into());
    }
    let text = &sql[statement.range.clone()];
    if fb_sql::classify(text).operation != Operation::Select {
        return Err(ONLY.into());
    }
    Ok(text.to_string())
}

/// `message` with the values it quotes taken out: a string literal, the
/// string a conversion failed on, the key a constraint found twice
pub(super) fn redact(message: &str) -> String {
    const KEY: &str = "Problematic key value is";
    let mut out = String::with_capacity(message.len());
    for (i, line) in message.lines().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        match line.find(KEY) {
            Some(at) => {
                out.push_str(&line[..at]);
                out.push_str(KEY);
                out.push_str(" (…)");
            }
            None => out.push_str(&redact_quotes(line)),
        }
    }
    out
}

fn redact_quotes(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        let closing = match c {
            '\'' => Some('\''),
            // `conversion error from string "…"`: the value, in double quotes
            '"' if out.to_ascii_lowercase().ends_with("string ") => Some('"'),
            _ => None,
        };
        let Some(closing) = closing else {
            out.push(c);
            continue;
        };
        out.push(c);
        out.push('…');
        while let Some(inner) = chars.next() {
            if inner == closing {
                // A doubled quote is one inside the literal
                if closing == '\'' && chars.peek() == Some(&'\'') {
                    chars.next();
                    continue;
                }
                out.push(closing);
                break;
            }
        }
    }
    out
}

/// Of the window's report on a query, only what the agent may know
pub(super) fn query_report(reply: &Value) -> Value {
    let mut report = Map::new();
    let ok = reply.get("ok").and_then(Value::as_bool).unwrap_or(false);
    report.insert("ran".into(), json!(ok));
    if let Some(error) = reply.get("error").and_then(Value::as_str) {
        report.insert("error".into(), json!(redact(error)));
    }
    if let Some(columns) = reply.get("columns").and_then(Value::as_array) {
        let columns: Vec<Value> = columns
            .iter()
            .map(|c| {
                json!({
                    "name": c.get("name").and_then(Value::as_str).unwrap_or(""),
                    "type": c.get("type").and_then(Value::as_str).unwrap_or(""),
                    "nullable": c.get("nullable").and_then(Value::as_bool).unwrap_or(true),
                })
            })
            .collect();
        report.insert("columns".into(), Value::Array(columns));
    }
    for key in ["rows", "prepareMs", "executeMs", "fetchMs"] {
        if let Some(n) = reply.get(key).and_then(Value::as_f64) {
            report.insert(key.into(), json!(n));
        }
    }
    if let Some(all) = reply.get("allFetched").and_then(Value::as_bool) {
        report.insert("allFetched".into(), json!(all));
    }
    for key in ["plan", "explainedPlan"] {
        if let Some(plan) = reply.get(key).and_then(Value::as_str) {
            report.insert(key.into(), json!(plan));
        }
    }
    if let Some(note) = reply.get("note").and_then(Value::as_str) {
        report.insert("note".into(), json!(note));
    }
    report.insert("rowsShownTo".into(), json!("the user only"));
    Value::Object(report)
}

fn run_query(app: &AppHandle, state: &AppState, args: &Value) -> Result<Value, String> {
    let (registration, _) = database(state, args)?;
    let sql = one_select(required(args, "sql")?)?;
    let agents = app.state::<Agents>();
    let reply =
        agents.ask(app, |request| Request::Run { request, registration: registration.id.clone(), sql }, RUN_WAIT)?;
    Ok(query_report(&reply))
}

/// The SELECT that counts what an UPDATE or DELETE would change: the rows
/// its own clauses pick (WHERE, PLAN, ORDER BY with ROWS), from its table
/// and alias, in a derived table so that ROWS limits the count as it limits
/// the change. RETURNING and SKIP LOCKED say nothing of which rows.
pub(super) fn count_query(sql: &str) -> Result<String, String> {
    use fb_sql::lexer::{tokenize, TokenKind};
    const ONLY: &str = "count_affected_rows counts for one UPDATE or DELETE.";
    let statements = fb_sql::split(sql);
    let [statement] = statements.as_slice() else {
        return Err(ONLY.into());
    };
    if statement.kind != StatementKind::Sql {
        return Err(ONLY.into());
    }
    if statement.unterminated_literal {
        return Err("The statement ends inside a string, a quoted name or a comment.".into());
    }
    let text = sql[statement.range.clone()].trim().trim_end_matches(';').trim_end();
    let operation = fb_sql::classify(text).operation;
    if !matches!(operation, Operation::Update | Operation::Delete) {
        return Err(ONLY.into());
    }
    let tokens: Vec<_> = tokenize(text).into_iter().filter(|t| !t.kind.is_trivia()).collect();
    if tokens.iter().any(|t| t.kind == TokenKind::Param) {
        return Err("The statement has parameters: give it the values to count with.".into());
    }
    let word = |i: usize| {
        tokens.get(i).filter(|t| t.kind == TokenKind::Word).map(|t| text[t.range.clone()].to_ascii_uppercase())
    };
    // How deep in parentheses each token is: only the statement's own
    // clauses count, not a subquery's
    let mut depths = Vec::with_capacity(tokens.len());
    let mut depth = 0i32;
    for t in &tokens {
        let punct = (t.kind == TokenKind::Punct).then(|| &text[t.range.clone()]);
        if punct == Some(")") {
            depth -= 1;
        }
        depths.push(depth);
        if punct == Some("(") {
            depth += 1;
        }
    }
    let top = |i: usize, words: &[&str]| depths[i] == 0 && word(i).is_some_and(|w| words.contains(&w.as_str()));
    let find = |from: usize, words: &[&str]| (from..tokens.len()).find(|&i| top(i, words));
    let start = |i: usize| tokens[i].range.start;

    if let Some(at) = find(0, &["WHERE"]) {
        if word(at + 1).as_deref() == Some("CURRENT") {
            return Err("WHERE CURRENT OF counts nothing: it is the row of a cursor.".into());
        }
    }
    let end = (0..tokens.len())
        .find(|&i| top(i, &["RETURNING"]) || (top(i, &["SKIP"]) && word(i + 1).as_deref() == Some("LOCKED")))
        .map_or(text.len(), start);

    let (source, clauses) = match operation {
        Operation::Delete => {
            if word(1).as_deref() != Some("FROM") {
                return Err(ONLY.into());
            }
            let from = find(2, &["WHERE", "PLAN", "ORDER", "ROWS"]).map_or(end, start).min(end);
            (&text[tokens.get(2).ok_or(ONLY)?.range.start..from], from)
        }
        _ => {
            let set = find(2, &["SET"]).ok_or(ONLY)?;
            let from = find(set + 1, &["WHERE", "PLAN", "ORDER", "ROWS"]).map_or(end, start).min(end);
            (&text[tokens.get(1).ok_or(ONLY)?.range.start..start(set)], from)
        }
    };
    let mut tail = text[clauses..end].trim();
    // A derived table takes ORDER BY only with ROWS; without it the order
    // changes nothing of which rows
    let rows = (0..tokens.len()).any(|i| start(i) >= clauses && start(i) < end && top(i, &["ROWS"]));
    if !rows {
        if let Some(order) = (0..tokens.len()).find(|&i| start(i) >= clauses && start(i) < end && top(i, &["ORDER"])) {
            tail = text[clauses..start(order)].trim();
        }
    }
    let source = source.trim();
    if source.is_empty() {
        return Err(ONLY.into());
    }
    let inner = if tail.is_empty() { format!("select 1 as n from {source}") } else { format!("select 1 as n from {source} {tail}") };
    Ok(format!("select count(*) from ({inner})"))
}

fn count_affected(state: &AppState, args: &Value) -> Result<Value, String> {
    let (_, conn) = database(state, args)?;
    let sql = count_query(required(args, "sql")?)?;
    let rows = count_on(&conn, &sql)?;
    Ok(json!({ "rows": rows, "counted": sql }))
}

/// Runs a count in a read-only transaction of its own, rolled back
fn count_on(conn: &Connection, sql: &str) -> Result<i64, String> {
    let failed = |e: fb_core::Error| redact(&e.to_string());
    let tx = conn.begin(fb_core::TxOptions { read_only: true, ..Default::default() }).map_err(failed)?;
    let counted = (|| {
        let executed = conn.execute(tx, sql, vec![], fb_core::ExecuteOptions { plan: false, performance: false })?;
        let Some(cursor) = executed.cursor else { return Ok(None) };
        let fetched = conn.fetch(cursor, 1)?;
        if !fetched.eof {
            conn.close_cursor(cursor)?;
        }
        Ok(fetched.rows.first().and_then(|row| row.first()).and_then(fb_core::Value::as_i64))
    })();
    // Nothing was written; the transaction only read
    let _ = conn.end(tx, fb_core::TxEnd::Rollback);
    counted.map_err(failed)?.ok_or_else(|| "The count gave no number.".to_string())
}

fn open_in_editor(app: &AppHandle, state: &AppState, args: &Value) -> Result<Value, String> {
    let (registration, _) = database(state, args)?;
    let sql = required(args, "sql")?.to_string();
    let title = text(args, "title").map(str::to_owned);
    let executive = match text(args, "in") {
        None | Some("editor") => false,
        Some("script_executive") => true,
        Some(other) => return Err(format!("`in` cannot be {other}: editor or script_executive.")),
    };
    let agents = app.state::<Agents>();
    let reply = agents.ask(
        app,
        |request| Request::Open { request, registration: registration.id.clone(), sql, title, executive },
        OPEN_WAIT,
    )?;
    match reply.get("error").and_then(Value::as_str) {
        Some(error) => Err(error.to_string()),
        None if executive => Ok(json!("Opened in a Script Executive for the user. It was not run.")),
        None => Ok(json!("Opened in an editor for the user. It was not run.")),
    }
}

fn shared_data(agents: &Agents, args: &Value) -> Result<Value, String> {
    let shared = agents.shared();
    match text(args, "id") {
        None if shared.is_empty() => Ok(json!("The user has shared nothing yet.")),
        None => Ok(Value::Array(
            shared
                .iter()
                .map(|s| {
                    json!({
                        "id": s.id,
                        "label": s.label,
                        "database": s.database,
                        "columns": s.columns.len(),
                        "rows": s.rows.len(),
                    })
                })
                .collect(),
        )),
        Some(id) => shared
            .into_iter()
            .find(|s| s.id == id)
            .map(|s| serde_json::to_value(s).unwrap_or_default())
            .ok_or_else(|| format!("Nothing was shared as {id}.")),
    }
}

/// The most of each list `get_activity` gives
const ACTIVITY_ROWS: usize = 10;
/// How much of a statement's text it gives
const SQL_CHARS: usize = 600;

/// A statement's text with its values taken out, cut short
fn without_literals(sql: Option<&str>) -> Option<String> {
    let sql = crate::trace::normalize(sql?);
    Some(if sql.chars().count() > SQL_CHARS {
        format!("{}…", sql.chars().take(SQL_CHARS).collect::<String>())
    } else {
        sql
    })
}

fn activity(state: &AppState, args: &Value) -> Result<Value, String> {
    let (_, conn) = database(state, args)?;
    activity_of(&conn)
}

fn activity_of(conn: &Connection) -> Result<Value, String> {
    use fb_meta::monitor;
    let failed = |e: fb_core::Error| e.to_string();
    let header = monitor::database(conn).map_err(failed)?;
    let attachments = monitor::attachments(conn).map_err(failed)?;
    let transactions = monitor::transactions(conn).map_err(failed)?;
    let statements = monitor::statements(conn, true).map_err(failed)?;
    // Memory is a figure more, not a reason to fail
    let memory = monitor::memory(conn).ok();

    let mut report = Map::new();
    if let Some(h) = header {
        report.insert(
            "database".into(),
            json!({
                "oldestTransaction": h.oldest_transaction,
                "oldestActive": h.oldest_active,
                "oldestSnapshot": h.oldest_snapshot,
                "nextTransaction": h.next_transaction,
                "nextMinusOldestActive": h.next_transaction - h.oldest_active,
                "oldestActiveMinusOldestTransaction": h.oldest_active - h.oldest_transaction,
                "sweepInterval": h.sweep_interval,
                "pageSize": h.page_size,
                "pages": h.pages,
                "pageBuffers": h.page_buffers,
                "ods": h.ods,
                "dialect": h.dialect,
                "forcedWrites": h.forced_writes,
                "readOnly": h.read_only,
                "shutdownMode": h.shutdown_mode,
                "backupState": h.backup_state,
            }),
        );
    }

    let users: Vec<_> = attachments.iter().filter(|a| !a.system).collect();
    let mut busiest = users.clone();
    busiest.sort_by_key(|a| std::cmp::Reverse(a.page_reads + a.page_fetches + a.page_writes));
    report.insert(
        "attachments".into(),
        json!({
            "count": users.len(),
            "active": users.iter().filter(|a| a.active).count(),
            "system": attachments.len() - users.len(),
            "busiest": busiest.iter().take(ACTIVITY_ROWS).map(|a| json!({
                "id": a.id,
                "isNStudio": a.current,
                "since": a.since,
                "pageReads": a.page_reads,
                "pageWrites": a.page_writes,
                "pageFetches": a.page_fetches,
                "pageMarks": a.page_marks,
                "seqReads": a.seq_reads,
                "idxReads": a.idx_reads,
                "inserts": a.inserts,
                "updates": a.updates,
                "deletes": a.deletes,
                "memoryUsed": a.memory_used,
            })).collect::<Vec<_>>(),
        }),
    );

    let mut active: Vec<_> = transactions.iter().filter(|t| t.active).collect();
    active.sort_by_key(|t| t.id);
    let mut isolations: Map<String, Value> = Map::new();
    for t in &active {
        let n = isolations.get(t.isolation).and_then(Value::as_u64).unwrap_or(0);
        isolations.insert(t.isolation.to_string(), json!(n + 1));
    }
    report.insert(
        "transactions".into(),
        json!({
            "active": active.len(),
            "readOnly": active.iter().filter(|t| t.read_only).count(),
            "byIsolation": isolations,
            "oldest": active.iter().take(ACTIVITY_ROWS).map(|t| json!({
                "id": t.id,
                "attachment": t.attachment,
                "since": t.since,
                "isolation": t.isolation,
                "readOnly": t.read_only,
                "autoCommit": t.auto_commit,
            })).collect::<Vec<_>>(),
        }),
    );

    report.insert(
        "runningStatements".into(),
        Value::Array(
            statements
                .iter()
                .take(ACTIVITY_ROWS * 2)
                .map(|s| {
                    json!({
                        "id": s.id,
                        "attachment": s.attachment,
                        "transaction": s.transaction,
                        "state": s.state,
                        "since": s.since,
                        "sql": without_literals(s.sql.as_deref()),
                        "textCut": s.cut,
                    })
                })
                .collect(),
        ),
    );

    if let Some(m) = memory {
        report.insert(
            "memory".into(),
            json!({
                "used": m.used,
                "peak": m.peak,
                "largestStatements": m.statements.iter().take(ACTIVITY_ROWS).map(|s| json!({
                    "id": s.id,
                    "attachment": s.attachment,
                    "used": s.used,
                    "peak": s.peak,
                    "sql": without_literals(s.sql.as_deref()),
                })).collect::<Vec<_>>(),
            }),
        );
    }
    Ok(Value::Object(report))
}

fn trace_summary(state: &AppState, args: &Value) -> Result<Value, String> {
    let (registration, _) = match text(args, "database") {
        Some(_) => registration(state, args)?,
        None => database(state, args).map(|(r, conn)| (r, Some(conn)))?,
    };
    let by = match text(args, "by") {
        None | Some("time") => "ms",
        Some(by @ ("runs" | "slowest" | "fetches" | "natural" | "records" | "memory")) => by,
        Some(other) => return Err(format!("`by` cannot be {other}.")),
    };
    let limit = args.get("limit").and_then(Value::as_u64).unwrap_or(15).clamp(1, 50) as usize;
    let summary = state.traces.summary(&registration.id, by, limit).ok_or_else(|| {
        format!(
            "No Activity Monitor session of {} is open. The user records one in its Activity Monitor, or opens a recorded one.",
            registration.name
        )
    })?;
    serde_json::to_value(summary).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_one_select_is_run() {
        assert_eq!(one_select("select * from rdb$database;").unwrap(), "select * from rdb$database");
        assert!(one_select("with x as (select 1 a from rdb$database) select * from x").is_ok());
        assert!(one_select("  -- why\n  SELECT 1 FROM RDB$DATABASE  ").is_ok());
        for refused in [
            "delete from t",
            "update t set a = 1",
            "insert into t values (1)",
            "execute block as begin end",
            "drop table t",
            "select 1 from rdb$database; delete from t",
            "set term ^ ;\nselect 1 from rdb$database^",
            "",
        ] {
            assert!(one_select(refused).is_err(), "{refused}");
        }
        assert!(one_select("select 'open from rdb$database").is_err());
    }

    #[test]
    fn the_values_an_error_quotes_are_taken_out() {
        assert_eq!(redact("conversion error from string \"12.345.678/0001-90\""), "conversion error from string \"…\"");
        assert_eq!(
            redact("violation of PRIMARY or UNIQUE KEY constraint \"PK_T\" on table \"T\"\n-Problematic key value is (\"ID\" = 5)"),
            "violation of PRIMARY or UNIQUE KEY constraint \"PK_T\" on table \"T\"\n-Problematic key value is (…)"
        );
        assert_eq!(redact("near 'it''s here' at line 1"), "near '…' at line 1");
        // Names in double quotes are metadata and stay
        assert_eq!(redact("Column unknown\n-\"NOME\""), "Column unknown\n-\"NOME\"");
    }

    #[test]
    fn a_query_report_holds_no_rows() {
        let reply = json!({
            "ok": true,
            "columns": [{ "name": "ID", "type": "INTEGER", "nullable": false, "relation": "T" }],
            "rows": 200,
            "allFetched": false,
            "prepareMs": 1.5,
            "plan": "PLAN (T NATURAL)",
            "data": [[1], [2]],
            "values": ["secret"],
        });
        let report = query_report(&reply);
        let text = report.to_string();
        assert!(!text.contains("secret") && !text.contains("\"data\""));
        assert_eq!(report["rows"], 200.0);
        assert_eq!(report["columns"][0], json!({ "name": "ID", "type": "INTEGER", "nullable": false }));
        assert_eq!(report["plan"], "PLAN (T NATURAL)");
    }

    fn registered(id: &str, name: &str) -> Registration {
        use crate::registry::{Auth, Engine, FirebirdSettings};
        use fb_core::Dialect;
        Registration {
            id: id.into(),
            name: name.into(),
            folder: vec!["Clients".into(), "South".into()],
            save_password: true,
            engine: Engine::Firebird(FirebirdSettings {
                host: "db.example".into(),
                port: 3050,
                database: "/srv/erp.fdb".into(),
                user: "SYSDBA".into(),
                role: String::new(),
                charset: "UTF8".into(),
                dialect: Dialect::D3,
                auth: Auth::Srp,
            }),
        }
    }

    #[test]
    fn a_database_is_found_by_id_then_by_name() {
        let list = vec![(registered("a1", "Sales"), ()), (registered("b2", "a1"), ())];
        assert_eq!(find(&list, "a1").unwrap().0.id, "a1");
        assert_eq!(find(&list, "SALES").unwrap().0.id, "a1");
        assert_eq!(find(&list, "b2").unwrap().0.name, "a1");
        assert!(find(&list, "nope").is_none());
    }

    #[test]
    fn a_registration_tells_the_agent_no_server_path_or_user() {
        let text = described(&registered("a1", "Sales"), None).to_string();
        assert!(text.contains("Clients / South") && text.contains("\"connected\":false"));
        for secret in ["db.example", "erp.fdb", "SYSDBA", "3050"] {
            assert!(!text.contains(secret), "{secret} in {text}");
        }
    }

    #[test]
    fn an_update_or_delete_is_counted_by_its_own_clauses() {
        let count = |sql: &str| count_query(sql).unwrap();
        assert_eq!(
            count("update cliente c set c.ativo = 0 where c.ultimo_pedido < '2020-01-01';"),
            "select count(*) from (select 1 as n from cliente c where c.ultimo_pedido < '2020-01-01')"
        );
        assert_eq!(
            count("DELETE FROM \"Pedido\" WHERE status = 'X' RETURNING id"),
            "select count(*) from (select 1 as n from \"Pedido\" WHERE status = 'X')"
        );
        assert_eq!(count("delete from log"), "select count(*) from (select 1 as n from log)");
        // A subquery's WHERE is not the statement's
        assert_eq!(
            count("update t set total = (select sum(v) from i where i.t = t.id) where t.dia = current_date"),
            "select count(*) from (select 1 as n from t where t.dia = current_date)"
        );
        // ROWS limits the count, and keeps its ORDER BY; without it the order goes
        assert_eq!(
            count("delete from fila order by criado rows 100"),
            "select count(*) from (select 1 as n from fila order by criado rows 100)"
        );
        assert_eq!(
            count("update fila set feito = 1 where feito = 0 order by criado"),
            "select count(*) from (select 1 as n from fila where feito = 0)"
        );
        assert_eq!(
            count("update t set a = 1 where b = 2 skip locked"),
            "select count(*) from (select 1 as n from t where b = 2)"
        );
    }

    /// Against the server the `FB_*` variables name, read only; skipped
    /// without them. The table is the first the database has, whatever it is.
    #[test]
    fn a_count_on_a_live_database_agrees_with_a_select() {
        let Some(params) = fb_core::ConnectParams::from_env() else {
            eprintln!("FB_DB not set: skipping live test");
            return;
        };
        let conn = Connection::open(&params).expect("connect");
        let first = conn
            .query("select first 1 trim(rdb$relation_name) from rdb$relations where rdb$system_flag = 0 and rdb$view_blr is null order by 1", vec![])
            .expect("tables");
        let Some(table) = first.rows.first().and_then(|r| r.first()).and_then(|v| v.as_str()).map(str::to_owned) else { return };
        let quoted = format!("\"{}\"", table.replace('"', "\"\""));
        let direct = conn.query(&format!("select count(*) from {quoted}"), vec![]).expect("count");
        let direct = direct.rows[0][0].as_i64().unwrap();
        assert_eq!(count_on(&conn, &count_query(&format!("delete from {quoted}")).unwrap()).unwrap(), direct);
        assert_eq!(count_on(&conn, &count_query(&format!("delete from {quoted} where 1 = 0")).unwrap()).unwrap(), 0);
        assert_eq!(count_on(&conn, &count_query(&format!("delete from {quoted} rows 1")).unwrap()).unwrap(), direct.min(1));
    }

    #[test]
    fn only_a_plain_update_or_delete_is_counted() {
        for refused in [
            "select * from t",
            "insert into t values (1)",
            "update or insert into t (id) values (1) matching (id)",
            "merge into t using s on t.id = s.id when matched then delete",
            "update t set a = 1 where id = ?",
            "delete from t where id = :id",
            "delete from t where current of c",
            "delete from t; delete from u",
        ] {
            assert!(count_query(refused).is_err(), "{refused} was counted");
        }
    }

    #[test]
    fn the_activity_shows_statements_without_their_values() {
        assert_eq!(
            without_literals(Some("select * from cliente where cpf = '123.456.789-00' and id = 42")).unwrap(),
            "select * from cliente where cpf = ? and id = ?"
        );
        assert_eq!(without_literals(None), None);
        assert!(without_literals(Some(&"x ".repeat(2_000))).unwrap().ends_with('…'));
    }

    /// Against the server the `FB_*` variables name, read only; skipped
    /// without them. Every statement it reports comes without its values.
    #[test]
    fn the_activity_of_a_live_database_reads_and_hides_values() {
        let Some(params) = fb_core::ConnectParams::from_env() else {
            eprintln!("FB_DB not set: skipping live test");
            return;
        };
        let conn = Connection::open(&params).expect("connect");
        let report = activity_of(&conn).expect("activity");
        assert!(
            report["database"]["nextTransaction"].as_i64().unwrap()
                >= report["database"]["oldestActive"].as_i64().unwrap()
        );
        assert!(report["attachments"]["count"].as_u64().unwrap() >= 1);
        assert!(report["attachments"]["busiest"].as_array().unwrap().iter().any(|a| a["isNStudio"] == true));
        for s in report["runningStatements"].as_array().unwrap() {
            let sql = s["sql"].as_str().unwrap_or("");
            assert!(!sql.contains('\''), "a string literal came through");
        }
        let text = report.to_string();
        for hidden in ["\"user\"", "\"address\"", "\"process\"", "\"host\""] {
            assert!(!text.contains(hidden), "{hidden} came through");
        }
    }

    #[test]
    fn the_window_saying_no_is_an_error() {
        assert_eq!(refusal(&json!({ "ok": true })), None);
        assert_eq!(
            refusal(&json!({ "ok": false, "error": "open transactions" })).as_deref(),
            Some("open transactions")
        );
        assert!(refusal(&json!({})).is_some());
    }

    #[test]
    fn every_tool_is_defined_and_every_read_only_one_exists() {
        let defined: Vec<String> = definitions().iter().map(|d| d["name"].as_str().unwrap().to_string()).collect();
        assert_eq!(defined, ALL);
        assert!(READ_ONLY.iter().all(|t| exists(t)));
        assert!(KINDS.iter().all(|k| serde_json::from_value::<ObjectKind>(json!(k)).is_ok()));
    }
}
