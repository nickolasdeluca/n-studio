//! The PSQL debugger's sessions. A debugged run is a thread of its own,
//! following the routine in a transaction of its own on the registration's
//! connection, and waiting, at each statement it stops at, for the UI to
//! say what next. What it sees goes out as [`DEBUG_STATE`] events.

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};

use fb_core::{Connection, TxEnd, TxOptions, Value};
use fb_meta::debug::{self, Outcome, Var, VarKind, Watch};
use fb_meta::ObjectKind;
use fb_sql::psql::{self, Body};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::commands::{blocking, AppState};
use crate::error::{AppError, AppResult};

pub const DEBUG_STATE: &str = "debug-state";

/// The rows a run's SUSPENDs are kept up to: past this they are counted
const KEEP: usize = 10_000;

enum Command {
    Step,
    Continue,
    Stop,
    End(TxEnd),
}

struct Session {
    commands: Sender<Command>,
    /// Lines, from 1
    breakpoints: Arc<Mutex<HashSet<usize>>>,
}

/// The runs being debugged, by the id the UI gave each
#[derive(Default)]
pub struct Debuggers(Mutex<HashMap<u64, Session>>);

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DebugVar {
    name: String,
    kind: VarKind,
    type_name: String,
    /// `None` is NULL
    value: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DebugNote {
    line: usize,
    what: String,
}

/// A routine opened to debug: its source, what it takes and gives, and
/// the statements the debugger will refuse
#[derive(Debug, Clone, Serialize)]
pub struct DebugTarget {
    source: String,
    inputs: Vec<DebugVar>,
    outputs: Vec<String>,
    notes: Vec<DebugNote>,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct DebugState {
    run: u64,
    /// `paused`, `finished` (its transaction waits to be ended), `ended`
    /// (committed or rolled back), `stopped` or `failed` (rolled back)
    state: &'static str,
    /// The statement it is at, or failed in: its first and last line
    line: Option<usize>,
    end_line: Option<usize>,
    variables: Vec<DebugVar>,
    /// The rows suspended since the last event
    rows: Vec<Vec<Option<String>>>,
    /// All of them, kept or not
    row_count: u64,
    /// What was passed over since the last event
    notes: Vec<DebugNote>,
    error: Option<String>,
}

fn line_of(source: &str, offset: usize) -> usize {
    source[..offset.min(source.len())].matches('\n').count() + 1
}

fn shown(vars: &[Var]) -> Vec<DebugVar> {
    vars.iter().map(|v| DebugVar { name: v.name.clone(), kind: v.kind, type_name: v.type_text.clone(), value: debug::shown(&v.value) }).collect()
}

fn read(conn: &Connection, kind: ObjectKind, name: &str) -> AppResult<(debug::Target, Body)> {
    let target = debug::target(conn, kind, name).map_err(|e| match e {
        fb_core::Error::Client { message } => AppError::Invalid { message },
        e => e.into(),
    })?;
    let body = psql::parse(&target.source).map_err(|e| AppError::Invalid {
        message: format!("The debugger cannot follow this routine. Line {}: {}.", line_of(&target.source, e.at), e.message),
    })?;
    Ok((target, body))
}

/// Reads a procedure or a function to debug
#[tauri::command]
pub async fn debug_open(app: AppHandle, id: String, kind: ObjectKind, name: String) -> AppResult<DebugTarget> {
    blocking(app, move |state| {
        let conn = state.session(&id)?;
        let (target, body) = read(&conn, kind, &name)?;
        let notes = body.unsupported().into_iter().map(|(at, what)| DebugNote { line: line_of(&target.source, at.start), what }).collect();
        Ok(DebugTarget {
            inputs: shown(&target.vars).into_iter().filter(|v| v.kind == VarKind::Input).collect(),
            outputs: target.vars.iter().filter(|v| v.kind == VarKind::Output).map(|v| v.name.clone()).collect(),
            source: target.source,
            notes,
        })
    })
    .await
}

/// The run as its thread sees it: what to tell the UI, and when to wait
struct Host {
    app: AppHandle,
    run: u64,
    source: String,
    commands: Receiver<Command>,
    breakpoints: Arc<Mutex<HashSet<usize>>>,
    stepping: bool,
    rows: Vec<Vec<Option<String>>>,
    row_count: u64,
    notes: Vec<DebugNote>,
}

impl Host {
    /// Tells the UI where the run is, with what gathered since it last heard
    fn tell(&mut self, state: &'static str, at: Option<&Range<usize>>, vars: &[Var], error: Option<String>) {
        let event = DebugState {
            run: self.run,
            state,
            line: at.map(|at| line_of(&self.source, at.start)),
            end_line: at.map(|at| line_of(&self.source, at.end.saturating_sub(1).max(at.start))),
            variables: shown(vars),
            rows: std::mem::take(&mut self.rows),
            row_count: self.row_count,
            notes: std::mem::take(&mut self.notes),
            error,
        };
        let _ = self.app.emit(DEBUG_STATE, event);
    }
}

impl Watch for Host {
    fn before(&mut self, at: &Range<usize>, vars: &[Var]) -> bool {
        // What was asked while it ran
        while let Ok(command) = self.commands.try_recv() {
            match command {
                Command::Step => self.stepping = true,
                Command::Continue => {}
                Command::Stop | Command::End(_) => return false,
            }
        }
        let line = line_of(&self.source, at.start);
        if !self.stepping && !self.breakpoints.lock().unwrap().contains(&line) {
            return true;
        }
        self.tell("paused", Some(at), vars, None);
        match self.commands.recv() {
            Ok(Command::Step) => {
                self.stepping = true;
                true
            }
            Ok(Command::Continue) => {
                self.stepping = false;
                true
            }
            // Its tab closed, or the app is going away
            Ok(Command::Stop | Command::End(_)) | Err(_) => false,
        }
    }

    fn row(&mut self, values: Vec<Value>) {
        self.row_count += 1;
        if self.row_count as usize <= KEEP {
            self.rows.push(values.iter().map(debug::shown).collect());
        }
    }

    fn skipped(&mut self, at: &Range<usize>, why: &str) {
        self.notes.push(DebugNote { line: line_of(&self.source, at.start), what: why.to_string() });
    }
}

/// The whole of a run, on its thread
fn debugged(conn: &Connection, mut host: Host, target: debug::Target, body: Body, inputs: Vec<Option<String>>) {
    let tx = match conn.begin(TxOptions::default()) {
        Ok(tx) => tx,
        Err(e) => return host.tell("failed", None, &[], Some(e.to_string())),
    };
    let mut vars = target.vars;
    let mut typed = inputs.into_iter();
    for var in vars.iter_mut().filter(|v| v.kind == VarKind::Input) {
        match debug::input(conn, tx, &var.type_text, typed.next().flatten()) {
            Ok(value) => var.value = value,
            Err(e) => {
                conn.end(tx, TxEnd::Rollback).ok();
                return host.tell("failed", None, &[], Some(format!("{}: {e}", var.name)));
            }
        }
    }
    let (vars, outcome) = debug::run(conn, tx, &target.source, &body, vars, &mut host);
    match outcome {
        Ok(Outcome::Finished) => {
            host.tell("finished", None, &vars, None);
            // What it did waits for a word; none coming, it is undone
            let how = match host.commands.recv() {
                Ok(Command::End(how)) => how,
                _ => TxEnd::Rollback,
            };
            let error = conn.end(tx, how).err().map(|e| e.to_string());
            host.tell("ended", None, &vars, error);
        }
        Ok(Outcome::Stopped) => {
            conn.end(tx, TxEnd::Rollback).ok();
            host.tell("stopped", None, &vars, None);
        }
        Err(failure) => {
            conn.end(tx, TxEnd::Rollback).ok();
            host.tell("failed", Some(&failure.at), &vars, Some(failure.message));
        }
    }
}

/// Starts debugging a routine: `inputs` are its parameters as typed, in
/// order, `None` for NULL. With `step` it stops at its first statement;
/// without, at the first of `breakpoints` it reaches. Returns once the run
/// is on its way: the rest is [`DEBUG_STATE`] events carrying `run`.
#[tauri::command]
pub async fn debug_start(
    app: AppHandle,
    id: String,
    run: u64,
    kind: ObjectKind,
    name: String,
    inputs: Vec<Option<String>>,
    breakpoints: Vec<usize>,
    step: bool,
) -> AppResult<()> {
    let events = app.clone();
    blocking(app, move |state| {
        let conn = state.session(&id)?;
        let (target, body) = read(&conn, kind, &name)?;
        let (commands, receiver) = channel();
        let breakpoints = Arc::new(Mutex::new(breakpoints.into_iter().collect::<HashSet<_>>()));
        state.debuggers.0.lock().unwrap().insert(run, Session { commands, breakpoints: breakpoints.clone() });
        let host = Host {
            app: events.clone(),
            run,
            source: target.source.clone(),
            commands: receiver,
            breakpoints,
            stepping: step,
            rows: Vec::new(),
            row_count: 0,
            notes: Vec::new(),
        };
        std::thread::spawn(move || {
            debugged(&conn, host, target, body, inputs);
            events.state::<AppState>().debuggers.0.lock().unwrap().remove(&run);
        });
        Ok(())
    })
    .await
}

/// Tells a run what next: `step`, `continue`, `stop`, or, once it has
/// finished, `commit` or `rollback`. `breakpoints`, when given, replace
/// the run's. `false` when there is no such run.
#[tauri::command]
pub fn debug_command(state: State<'_, AppState>, run: u64, command: String, breakpoints: Option<Vec<usize>>) -> AppResult<bool> {
    let sessions = state.debuggers.0.lock().unwrap();
    let Some(session) = sessions.get(&run) else { return Ok(false) };
    if let Some(lines) = breakpoints {
        *session.breakpoints.lock().unwrap() = lines.into_iter().collect();
    }
    let command = match command.as_str() {
        "step" => Command::Step,
        "continue" => Command::Continue,
        "stop" => Command::Stop,
        "commit" => Command::End(TxEnd::Commit),
        "rollback" => Command::End(TxEnd::Rollback),
        // Only the breakpoints changed
        "" => return Ok(true),
        other => return Err(AppError::Invalid { message: format!("The debugger has no command {other}.") }),
    };
    Ok(session.commands.send(command).is_ok())
}
