//! The profiler of Firebird 5 (`RDB$PROFILER`): what the statements an
//! attachment runs while a session of it is open spend their time on, by
//! routine, by line of PSQL and by record source of each plan.
//!
//! A session belongs to the attachment that starts it, and what it measures
//! is written, when it finishes, to tables of the database itself
//! (`PLG$PROF_*`, created by the default profiler plugin the first time it
//! is used): profiling writes to the database, so it runs only when asked,
//! and a session's data is deleted only when asked. Times are nanoseconds.

use fb_core::{Connection, ExecuteOptions, Result, TxEnd, TxOptions, Value};
use serde::Serialize;

use crate::row::{inline, rows, Row};

/// A session, as `PLG$PROF_SESSIONS` keeps it
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProfileSession {
    pub id: i64,
    pub attachment: i64,
    pub user: Option<String>,
    pub description: Option<String>,
    pub started: Option<String>,
    /// None while it is open, or when it was cancelled
    pub finished: Option<String>,
}

/// A statement the session saw run: a top-level statement, or the routine
/// or trigger one called
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProfiledStatement {
    pub id: i64,
    pub parent: Option<i64>,
    /// `BLOCK`, `PROCEDURE`, `FUNCTION` or `TRIGGER`
    pub kind: Option<String>,
    pub package: Option<String>,
    pub routine: Option<String>,
    /// A top-level statement's text, as far as it comes with the row
    pub sql: Option<String>,
    /// Times it ran, and what they took together
    pub runs: i64,
    pub total: i64,
}

/// One line and column of PSQL, over every run of its statement
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProfiledLine {
    pub statement: i64,
    pub line: i64,
    pub column: i64,
    pub count: i64,
    pub min: i64,
    pub max: i64,
    pub total: i64,
}

/// One step of a plan (a record source), over every run of its statement
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProfiledSource {
    pub statement: i64,
    pub cursor: i64,
    /// A named cursor's name; the line of an unnamed one, in PSQL
    pub cursor_name: Option<String>,
    pub cursor_line: Option<i64>,
    pub id: i64,
    pub parent: Option<i64>,
    /// How deep it is in the plan: 0 the source the cursor reads
    pub level: i64,
    /// `Table "CUSTOMERS" Access By ID`, as an explained plan says it
    pub access_path: Option<String>,
    pub opens: i64,
    pub fetches: i64,
    /// Opening and fetching together
    pub total: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Profile {
    pub session: ProfileSession,
    /// The costliest first
    pub statements: Vec<ProfiledStatement>,
    pub lines: Vec<ProfiledLine>,
    /// In each statement's plan order
    pub sources: Vec<ProfiledSource>,
}

/// Lines and record sources kept in a report: the costliest
const LINES: usize = 300;
const SOURCES: usize = 2000;

/// Runs `sql` in a read-write transaction of its own, and its output row
fn run(conn: &Connection, sql: &str, params: Vec<Value>) -> Result<Vec<Value>> {
    let tx = conn.begin(TxOptions::default())?;
    match conn.execute(tx, sql, params, ExecuteOptions { plan: false, performance: false }) {
        Ok(done) => {
            conn.end(tx, TxEnd::Commit)?;
            Ok(done.rows.into_iter().next().unwrap_or_default())
        }
        Err(e) => {
            conn.end(tx, TxEnd::Rollback).ok();
            Err(e)
        }
    }
}

/// Starts a session on the attachment `conn` is: what it runs from now on is
/// measured. Its id, by which it is found once it finishes.
pub fn start(conn: &Connection, description: &str) -> Result<i64> {
    let row = run(
        conn,
        "execute block (description varchar(255) = ?) returns (id bigint) as \
         begin id = rdb$profiler.start_session(:description); end",
        vec![Value::Text(description.to_string())],
    )?;
    Ok(row.first().and_then(Value::as_i64).unwrap_or(0))
}

/// Finishes the open session, writing down what it measured
pub fn finish(conn: &Connection) -> Result<()> {
    run(conn, "execute procedure rdb$profiler.finish_session(true)", vec![]).map(drop)
}

/// Ends the open session without writing anything down
pub fn cancel(conn: &Connection) -> Result<()> {
    run(conn, "execute procedure rdb$profiler.cancel_session", vec![]).map(drop)
}

/// Whether the profiler's tables are there: not before its first session
fn has_tables(conn: &Connection) -> Result<bool> {
    let found = rows(conn, "select 1 from rdb$relations where rdb$relation_name = 'PLG$PROF_SESSIONS'", vec![])?;
    Ok(!found.is_empty())
}

fn session(r: &Row) -> ProfileSession {
    ProfileSession {
        id: r.int(0).unwrap_or(0),
        attachment: r.int(1).unwrap_or(0),
        user: r.text(2),
        description: r.text(3),
        started: r.text(4),
        finished: r.text(5),
    }
}

const SESSION_COLUMNS: &str = "profile_id, attachment_id, user_name, description, \
     cast(cast(start_timestamp as timestamp) as varchar(30)), cast(cast(finish_timestamp as timestamp) as varchar(30))";

/// The sessions written down in the database, the latest first
pub fn sessions(conn: &Connection) -> Result<Vec<ProfileSession>> {
    if !has_tables(conn)? {
        return Ok(Vec::new());
    }
    let sql = format!("select first 100 {SESSION_COLUMNS} from plg$prof_sessions order by profile_id desc");
    Ok(rows(conn, &sql, vec![])?.iter().map(session).collect())
}

/// What a session measured. None when there is no such session.
pub fn report(conn: &Connection, id: i64) -> Result<Option<Profile>> {
    if !has_tables(conn)? {
        return Ok(None);
    }
    let key = || vec![Value::Integer(id)];
    let sql = format!("select {SESSION_COLUMNS} from plg$prof_sessions where profile_id = ?");
    let Some(found) = rows(conn, &sql, key())?.first().map(session) else { return Ok(None) };

    let mut statements: Vec<ProfiledStatement> = rows(
        conn,
        &format!(
            "select statement_id, parent_statement_id, statement_type, package_name, routine_name, {} \
             from plg$prof_statements where profile_id = ?",
            inline("sql_text")
        ),
        key(),
    )?
    .iter()
    .map(|r| ProfiledStatement {
        id: r.int(0).unwrap_or(0),
        parent: r.int(1),
        kind: r.text(2),
        package: r.text(3),
        routine: r.text(4),
        sql: r.text(5),
        runs: 0,
        total: 0,
    })
    .collect();
    let runs = rows(
        conn,
        "select statement_id, count(*), sum(total_elapsed_time) from plg$prof_requests \
         where profile_id = ? group by statement_id",
        key(),
    )?;
    for r in &runs {
        if let Some(s) = statements.iter_mut().find(|s| Some(s.id) == r.int(0)) {
            s.runs = r.int(1).unwrap_or(0);
            s.total = r.int(2).unwrap_or(0);
        }
    }
    statements.sort_by(|a, b| b.total.cmp(&a.total).then(a.id.cmp(&b.id)));

    let lines = rows(
        conn,
        &format!(
            "select first {LINES} statement_id, line_num, column_num, counter, min_elapsed_time, max_elapsed_time, \
             total_elapsed_time from plg$prof_psql_stats_view where profile_id = ? \
             order by total_elapsed_time desc, statement_id, line_num, column_num"
        ),
        key(),
    )?
    .iter()
    .map(|r| ProfiledLine {
        statement: r.int(0).unwrap_or(0),
        line: r.int(1).unwrap_or(0),
        column: r.int(2).unwrap_or(0),
        count: r.int(3).unwrap_or(0),
        min: r.int(4).unwrap_or(0),
        max: r.int(5).unwrap_or(0),
        total: r.int(6).unwrap_or(0),
    })
    .collect();

    let sources = rows(
        conn,
        &format!(
            "select first {SOURCES} statement_id, cursor_id, cursor_name, cursor_line_num, record_source_id, \
             parent_record_source_id, level, {}, open_counter, fetch_counter, open_fetch_total_elapsed_time \
             from plg$prof_record_source_stats_view where profile_id = ? \
             order by statement_id, cursor_id, record_source_id",
            inline("access_path")
        ),
        key(),
    )?
    .iter()
    .map(|r| ProfiledSource {
        statement: r.int(0).unwrap_or(0),
        cursor: r.int(1).unwrap_or(0),
        cursor_name: r.text(2),
        cursor_line: r.int(3),
        id: r.int(4).unwrap_or(0),
        parent: r.int(5),
        level: r.int(6).unwrap_or(0),
        access_path: r.text(7),
        opens: r.int(8).unwrap_or(0),
        fetches: r.int(9).unwrap_or(0),
        total: r.int(10).unwrap_or(0),
    })
    .collect();

    Ok(Some(Profile { session: found, statements, lines, sources }))
}

/// Deletes what a session wrote down; its statements, lines and record
/// sources go with it
pub fn delete(conn: &Connection, id: i64) -> Result<()> {
    run(conn, "delete from plg$prof_sessions where profile_id = ?", vec![Value::Integer(id)]).map(drop)
}
