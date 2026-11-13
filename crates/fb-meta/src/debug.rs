//! A stored routine run one statement at a time. The server has no
//! debugger, so this is an emulator: the routine's source is taken apart
//! ([`fb_sql::psql`]), the flow is followed here, and every expression and
//! SQL statement is sent to the server, in the debugged run's own
//! transaction, with the variables it names as parameters.
//!
//! So what runs is not the compiled routine but its source read again, and
//! what the emulator does not follow it refuses rather than guesses: hand
//! opened cursors, autonomous transactions, EXECUTE STATEMENT with options,
//! triggers' NEW and OLD. Handlers for SQLCODE and GDSCODE never match.

use fb_core::{Connection, Error, ExecuteOptions, Result, TxId, Value};
use fb_sql::psql::{bind, Body, Handler, Node, Span, Stmt};
use serde::Serialize;

use crate::ddl::Context;
use crate::ident::quote;
use crate::routine::{function, procedure, Param};
use crate::ObjectKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VarKind {
    Input,
    Output,
    Local,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Var {
    pub name: String,
    pub kind: VarKind,
    /// As a CAST takes it
    pub type_text: String,
    pub value: Value,
}

/// A routine as the debugger needs it
#[derive(Debug, Clone)]
pub struct Target {
    pub source: String,
    /// Inputs then outputs, none with a value yet. A function's result is
    /// an output named `RESULT`.
    pub vars: Vec<Var>,
}

/// Why a run ended before its end
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    /// The statement it happened in
    pub at: Span,
    pub message: String,
    /// The exception raised, when the routine raised it itself
    pub exception: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Finished,
    Stopped,
}

/// Who is watching the run
pub trait Watch {
    /// Before each statement, with the variables as they are; `false`
    /// stops the run
    fn before(&mut self, at: &Span, vars: &[Var]) -> bool;
    /// A SUSPEND: the outputs as they are
    fn row(&mut self, values: Vec<Value>);
    /// A statement that was passed over, and why
    fn skipped(&mut self, _at: &Span, _why: &str) {}
}

/// The type of a parameter, as a CAST takes it
fn cast_type(p: &Param, ctx: &Context) -> String {
    let q = |name: &str| quote(name, ctx.dialect);
    match (&p.type_of_column, &p.domain) {
        (Some((table, column)), _) => format!("TYPE OF COLUMN {}.{}", q(table), q(column)),
        (None, Some(domain)) => q(domain),
        (None, None) => p.data_type.declaration(ctx.dialect, &ctx.charset),
    }
}

/// Reads a procedure or a function to debug
pub fn target(conn: &Connection, kind: ObjectKind, name: &str) -> Result<Target> {
    let routine = match kind {
        ObjectKind::Procedure => procedure(conn, name)?,
        ObjectKind::Function => function(conn, name)?,
        _ => return Err(Error::Client { message: "Only procedures and functions can be debugged.".into() }),
    };
    if routine.external.is_some() {
        return Err(Error::Client { message: format!("{name} is an external routine: it has no PSQL to follow.") });
    }
    let Some(source) = routine.source.filter(|s| !s.trim().is_empty()) else {
        return Err(Error::Client { message: format!("The source of {name} is not in the database.") });
    };
    let ctx = Context::of(conn.info());
    let var = |p: &Param, kind: VarKind| Var {
        name: if p.name.is_empty() { "RESULT".to_string() } else { p.name.clone() },
        kind,
        type_text: cast_type(p, &ctx),
        value: Value::Null,
    };
    let vars = routine.inputs.iter().map(|p| var(p, VarKind::Input)).chain(routine.outputs.iter().map(|p| var(p, VarKind::Output))).collect();
    Ok(Target { source, vars })
}

/// A value as it is shown; `None` is NULL
pub fn shown(value: &Value) -> Option<String> {
    match value {
        Value::Null => None,
        Value::Integer(i) => Some(i.to_string()),
        Value::Decimal(d) => Some(d.to_string()),
        Value::Float(f) => Some(format!("{f:?}")),
        Value::Boolean(b) => Some(if *b { "TRUE" } else { "FALSE" }.to_string()),
        Value::Text(s) => Some(s.clone()),
        Value::Binary(b) => Some(b.iter().map(|byte| format!("{byte:02X}")).collect()),
        Value::Timestamp(t) => Some(t.to_string()),
        Value::BlobRef { .. } => Some("(blob)".to_string()),
    }
}

/// What a statement leaves the run to do next
enum Flow {
    Next,
    Exit,
    Leave(Option<String>),
    Continue(Option<String>),
    Stopped,
}

type Step<T> = std::result::Result<T, Failure>;

const PLAIN: ExecuteOptions = ExecuteOptions { plan: false, performance: false };

struct Machine<'a> {
    conn: &'a Connection,
    tx: TxId,
    source: &'a str,
    vars: Vec<Var>,
    watch: &'a mut dyn Watch,
    /// What the last statement changed, or read: ROW_COUNT
    row_count: i64,
    savepoints: u32,
    /// The error a handler is running for, which a bare EXCEPTION raises again
    handling: Option<Failure>,
}

fn failed(at: &Span, e: Error) -> Failure {
    Failure { at: at.clone(), message: e.to_string(), exception: None }
}

/// Whether a server's message names an exception: a called routine's is
/// only known by what the server says of it
fn mentions(message: &str, name: &str) -> bool {
    message.split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '$')).any(|word| word.eq_ignore_ascii_case(name))
}

impl Machine<'_> {
    /// A name's type and value, if it is a variable
    fn lookup(&self, name: &str) -> Option<(&str, Value)> {
        match self.vars.iter().find(|v| v.name == name) {
            Some(v) => Some((&v.type_text, v.value.clone())),
            None if name == "ROW_COUNT" => Some(("INTEGER", Value::Integer(self.row_count))),
            None => None,
        }
    }

    /// A piece of the source for the server, and its parameters' values
    fn bound(&self, span: &Span, colon_only: bool) -> (String, Vec<Value>) {
        let b = bind(&self.source[span.clone()], colon_only, |name| self.lookup(name).map(|(t, _)| format!("CAST(? AS {t})")));
        let values = b.names.iter().map(|n| self.lookup(n).map_or(Value::Null, |(_, v)| v)).collect();
        (b.sql, values)
    }

    /// Runs a statement and gives its row, when one is `wanted` and it has
    /// one. More than one is an error, as it is in PSQL.
    fn row(&mut self, at: &Span, sql: &str, params: Vec<Value>, wanted: bool) -> Step<Option<Vec<Value>>> {
        let executed = self.conn.execute(self.tx, sql, params, PLAIN).map_err(|e| failed(at, e))?;
        let r = &executed.records;
        self.row_count = r.inserted + r.updated + r.deleted;
        let Some(cursor) = executed.cursor else { return Ok(executed.rows.into_iter().next()) };
        if !wanted {
            self.conn.close_cursor(cursor).map_err(|e| failed(at, e))?;
            return Ok(None);
        }
        let fetched = self.conn.fetch(cursor, 2).map_err(|e| failed(at, e))?;
        if !fetched.eof {
            self.conn.close_cursor(cursor).map_err(|e| failed(at, e))?;
        }
        if fetched.rows.len() > 1 {
            return Err(Failure { at: at.clone(), message: "multiple rows in singleton select".into(), exception: None });
        }
        self.row_count = fetched.rows.len() as i64;
        Ok(fetched.rows.into_iter().next())
    }

    /// What an expression comes to, as `cast` when there is one
    fn value(&mut self, at: &Span, expr: &Span, cast: Option<&str>) -> Step<Value> {
        let (text, params) = self.bound(expr, false);
        // On lines of its own: a comment at its end must not take the rest
        let sql = match cast {
            Some(t) => format!("SELECT CAST((\n{text}\n) AS {t}) FROM RDB$DATABASE"),
            None => format!("SELECT (\n{text}\n) FROM RDB$DATABASE"),
        };
        let kept = self.row_count;
        let row = self.row(at, &sql, params, true)?;
        self.row_count = kept;
        Ok(row.and_then(|r| r.into_iter().next()).unwrap_or(Value::Null))
    }

    fn truth(&mut self, at: &Span, cond: &Span) -> Step<bool> {
        let (text, params) = self.bound(cond, false);
        let sql = format!("SELECT CASE WHEN\n{text}\nTHEN 1 ELSE 0 END FROM RDB$DATABASE");
        let kept = self.row_count;
        let row = self.row(at, &sql, params, true)?;
        self.row_count = kept;
        Ok(row.and_then(|r| r.into_iter().next()).and_then(|v| v.as_i64()) == Some(1))
    }

    fn set(&mut self, at: &Span, name: &str, value: Value) -> Step<()> {
        match self.vars.iter_mut().find(|v| v.name == name) {
            Some(v) => {
                v.value = value;
                Ok(())
            }
            None => Err(Failure { at: at.clone(), message: format!("{name} is not a variable of the routine."), exception: None }),
        }
    }

    fn set_all(&mut self, at: &Span, into: &[String], row: Vec<Value>) -> Step<()> {
        if into.len() != row.len() {
            let message = format!("{} values for {} variables.", row.len(), into.len());
            return Err(Failure { at: at.clone(), message, exception: None });
        }
        into.iter().zip(row).try_for_each(|(name, value)| self.set(at, name, value))
    }

    /// A statement of the emulator's own, in the run's transaction
    fn plain(&mut self, at: &Span, sql: &str) -> Step<()> {
        self.conn.execute(self.tx, sql, vec![], PLAIN).map(|_| ()).map_err(|e| failed(at, e))
    }

    fn sql(&mut self, at: &Span, sql: &str, params: Vec<Value>, into: &[String]) -> Step<()> {
        match self.row(at, sql, params, !into.is_empty())? {
            Some(row) if !into.is_empty() => self.set_all(at, into, row),
            _ => Ok(()),
        }
    }

    fn raise(&mut self, at: &Span, name: &Option<String>, message: &Option<Span>) -> Step<Flow> {
        let Some(name) = name else {
            let again = self.handling.clone();
            return Err(again.unwrap_or_else(|| Failure { at: at.clone(), message: "No error is being handled to raise again.".into(), exception: None }));
        };
        let text = match message {
            Some(expr) => shown(&self.value(at, expr, None)?),
            None => {
                let sql = "select rdb$message from rdb$exceptions where rdb$exception_name = ?";
                let found = self.row(at, sql, vec![Value::Text(name.clone())], true)?;
                match found {
                    Some(row) => row.first().and_then(shown).map(|m| m.trim_end().to_string()),
                    None => return Err(Failure { at: at.clone(), message: format!("There is no exception named {name}."), exception: None }),
                }
            }
        };
        let message = match text {
            Some(text) => format!("exception {name}: {text}"),
            None => format!("exception {name}"),
        };
        Err(Failure { at: at.clone(), message, exception: Some(name.clone()) })
    }

    /// A block with handlers: what it did is undone before one runs
    fn guarded(&mut self, at: &Span, body: &[Node], handlers: &[Handler]) -> Step<Flow> {
        self.savepoints += 1;
        let savepoint = format!("NS_DEBUG_{}", self.savepoints);
        self.plain(at, &format!("SAVEPOINT {savepoint}"))?;
        match self.sequence(body) {
            Err(failure) => {
                let caught = handlers.iter().find(|h| {
                    h.any || h.exceptions.iter().any(|e| failure.exception.as_ref() == Some(e) || (failure.exception.is_none() && mentions(&failure.message, e)))
                });
                let Some(handler) = caught else { return Err(failure) };
                self.plain(at, &format!("ROLLBACK TO SAVEPOINT {savepoint}"))?;
                let before = self.handling.replace(failure);
                let flow = self.exec(&handler.body);
                self.handling = before;
                flow
            }
            done => {
                // Its work stays; only the name is given back
                self.plain(at, &format!("RELEASE SAVEPOINT {savepoint} ONLY")).ok();
                done
            }
        }
    }

    fn sequence(&mut self, body: &[Node]) -> Step<Flow> {
        for node in body {
            match self.exec(node)? {
                Flow::Next => {}
                other => return Ok(other),
            }
        }
        Ok(Flow::Next)
    }

    /// What a loop does with what its body left: `Some` ends the loop
    fn looped(label: &Option<String>, flow: Flow) -> Option<Flow> {
        let own = |l: &Option<String>| l.is_none() || l == label;
        match flow {
            Flow::Next => None,
            Flow::Continue(l) if own(&l) => None,
            Flow::Leave(l) if own(&l) => Some(Flow::Next),
            other => Some(other),
        }
    }

    fn exec(&mut self, node: &Node) -> Step<Flow> {
        let at = &node.span;
        if !matches!(node.stmt, Stmt::Block { .. }) && !self.watch.before(at, &self.vars) {
            return Ok(Flow::Stopped);
        }
        match &node.stmt {
            Stmt::Block { body, handlers } if handlers.is_empty() => self.sequence(body),
            Stmt::Block { body, handlers } => self.guarded(at, body, handlers),
            Stmt::Assign { target, expr } => {
                let Some(cast) = self.vars.iter().find(|v| &v.name == target).map(|v| v.type_text.clone()) else {
                    return Err(Failure { at: at.clone(), message: format!("{target} is not a variable of the routine."), exception: None });
                };
                let value = self.value(at, expr, Some(&cast))?;
                self.set(at, target, value)?;
                Ok(Flow::Next)
            }
            Stmt::If { cond, then, otherwise } => {
                if self.truth(at, cond)? {
                    self.exec(then)
                } else {
                    otherwise.as_ref().map_or(Ok(Flow::Next), |o| self.exec(o))
                }
            }
            Stmt::While { label, cond, body } => {
                let mut first = true;
                loop {
                    // Each turn stops at the loop's head again
                    if !first && !self.watch.before(at, &self.vars) {
                        return Ok(Flow::Stopped);
                    }
                    first = false;
                    if !self.truth(at, cond)? {
                        return Ok(Flow::Next);
                    }
                    if let Some(flow) = Self::looped(label, self.exec(body)?) {
                        return Ok(flow);
                    }
                }
            }
            Stmt::ForSelect { label, select, into, body } => {
                let (sql, params) = self.bound(select, true);
                let executed = self.conn.execute(self.tx, &sql, params, PLAIN).map_err(|e| failed(at, e))?;
                let Some(cursor) = executed.cursor else {
                    return Err(Failure { at: at.clone(), message: "The loop's statement returns no rows to loop over.".into(), exception: None });
                };
                let mut open = true;
                let mut turn = || -> Step<Option<Flow>> {
                    let fetched = self.conn.fetch(cursor, 1).map_err(|e| failed(at, e))?;
                    open = !fetched.eof;
                    let Some(row) = fetched.rows.into_iter().next() else { return Ok(Some(Flow::Next)) };
                    self.set_all(at, into, row)?;
                    self.row_count = 1;
                    let flow = self.exec(body)?;
                    if let Some(flow) = Self::looped(label, flow) {
                        return Ok(Some(flow));
                    }
                    if !open {
                        return Ok(Some(Flow::Next));
                    }
                    Ok((!self.watch.before(at, &self.vars)).then_some(Flow::Stopped))
                };
                let ended = loop {
                    match turn() {
                        Ok(None) => {}
                        Ok(Some(flow)) => break Ok(flow),
                        Err(failure) => break Err(failure),
                    }
                };
                if open {
                    self.conn.close_cursor(cursor).ok();
                }
                ended
            }
            Stmt::Sql { text, into } => {
                let (sql, params) = self.bound(text, true);
                self.sql(at, &sql, params, into)?;
                Ok(Flow::Next)
            }
            Stmt::ExecuteStatement { text, into } => {
                let Some(sql) = shown(&self.value(at, text, None)?) else {
                    return Err(Failure { at: at.clone(), message: "The statement to execute is NULL.".into(), exception: None });
                };
                self.sql(at, &sql, vec![], into)?;
                Ok(Flow::Next)
            }
            Stmt::Suspend => {
                let outputs = self.vars.iter().filter(|v| v.kind == VarKind::Output).map(|v| v.value.clone()).collect();
                self.watch.row(outputs);
                Ok(Flow::Next)
            }
            Stmt::Exit => Ok(Flow::Exit),
            Stmt::Leave(label) => Ok(Flow::Leave(label.clone())),
            Stmt::Continue(label) => Ok(Flow::Continue(label.clone())),
            Stmt::Raise { name, message } => self.raise(at, name, message),
            Stmt::Return(expr) => {
                let Some((name, cast)) = self.vars.iter().find(|v| v.kind == VarKind::Output).map(|v| (v.name.clone(), v.type_text.clone())) else {
                    return Err(Failure { at: at.clone(), message: "RETURN in a routine that returns nothing.".into(), exception: None });
                };
                let value = self.value(at, expr, Some(&cast))?;
                self.set(at, &name, value)?;
                Ok(Flow::Exit)
            }
            Stmt::Skip(why) => {
                self.watch.skipped(at, why);
                Ok(Flow::Next)
            }
            Stmt::Unsupported(what) => {
                Err(Failure { at: at.clone(), message: format!("The debugger cannot run this: {what}."), exception: None })
            }
        }
    }
}

/// A value typed for an input, as the input's type; `None` is NULL
pub fn input(conn: &Connection, tx: TxId, type_text: &str, text: Option<String>) -> Result<Value> {
    let Some(text) = text else { return Ok(Value::Null) };
    let sql = format!("SELECT CAST(? AS {type_text}) FROM RDB$DATABASE");
    let executed = conn.execute(tx, &sql, vec![Value::Text(text)], PLAIN)?;
    let Some(cursor) = executed.cursor else { return Ok(Value::Null) };
    let fetched = conn.fetch(cursor, 1)?;
    if !fetched.eof {
        conn.close_cursor(cursor)?;
    }
    Ok(fetched.rows.into_iter().next().and_then(|r| r.into_iter().next()).unwrap_or(Value::Null))
}

/// Runs a parsed routine in `tx`, which is left open: whoever debugs
/// decides what becomes of what it did. `vars` are the inputs, with their
/// values, and the outputs; the variables come back as the run left them.
pub fn run(conn: &Connection, tx: TxId, source: &str, body: &Body, vars: Vec<Var>, watch: &mut dyn Watch) -> (Vec<Var>, std::result::Result<Outcome, Failure>) {
    let mut machine = Machine { conn, tx, source, vars, watch, row_count: 0, savepoints: 0, handling: None };
    let outcome = (|| {
        for declared in &body.variables {
            let value = match &declared.default {
                Some(expr) => machine.value(expr, expr, Some(&declared.type_text))?,
                None => Value::Null,
            };
            machine.vars.push(Var { name: declared.name.clone(), kind: VarKind::Local, type_text: declared.type_text.clone(), value });
        }
        match machine.exec(&body.block)? {
            Flow::Stopped => Ok(Outcome::Stopped),
            _ => Ok(Outcome::Finished),
        }
    })();
    (machine.vars, outcome)
}
