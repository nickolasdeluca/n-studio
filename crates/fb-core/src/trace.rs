//! A trace session: the server writing down what it runs, as it runs it.
//!
//! This is the user trace session of the services manager (what
//! `fbtracemgr` starts), not the audit trace of `firebird.conf`. The server
//! formats each event as text and keeps it in a buffer of its own until it
//! is read (`MaxUserTraceLogSize`, 10 MB unless configured). A reader that
//! falls behind fills that buffer, and the server then drops events and
//! says so in the text: reading without pause makes that rare, and nothing
//! makes it impossible.
//!
//! Who sees what is the server's rule: SYSDBA and administrators see every
//! attachment, anyone else only their own.

mod parse;

use std::fmt::Write as _;
use std::sync::atomic::{AtomicBool, Ordering};

use rsfbclient_core::ibase;
use serde::{Deserialize, Serialize};

use crate::service::{attach, parse_answer, timeout_item, Spb, INFO_TO_EOF};
use crate::{ConnectParams, Connection, Error, Result};

pub use parse::{TableCounts, TraceEvent, TraceItem, TraceOutcome, TraceReader};

/// What a session writes down
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TraceOptions {
    /// Attachments made and ended
    pub connections: bool,
    /// Transactions started, committed and rolled back
    pub transactions: bool,
    /// Statements as they finish, with what they cost
    pub statements: bool,
    /// Procedures, functions and triggers as they finish
    pub routines: bool,
    /// The start of each statement and routine too: what tells which
    /// statement ran inside which procedure
    pub starts: bool,
    /// Statements prepared and freed, and (Firebird 5) routines compiled
    pub prepares: bool,
    /// Context variables as they are set
    pub context: bool,
    /// Errors and warnings
    pub errors: bool,
    pub sweep: bool,
    pub plans: bool,
    /// The plan as a tree (Firebird 3 and later), in place of the short one
    pub explained_plans: bool,
    /// Time and page counters, and the records read and written per table
    pub performance: bool,
    /// What finished faster than this, in milliseconds, is left out. 0
    /// leaves nothing out.
    pub threshold_ms: u32,
    /// Only statements matching this (SIMILAR TO, the whole text)
    pub include: String,
    /// No statement matching this
    pub exclude: String,
    /// Every database of the server, not only the connected one
    pub whole_server: bool,
    /// Characters of a statement kept
    pub max_sql: u32,
    /// Characters of a parameter kept
    pub max_argument: u32,
    /// Parameters kept
    pub max_arguments: u32,
}

impl Default for TraceOptions {
    fn default() -> Self {
        TraceOptions {
            connections: true,
            transactions: true,
            statements: true,
            routines: true,
            starts: true,
            prepares: false,
            context: false,
            errors: true,
            sweep: true,
            plans: true,
            explained_plans: false,
            performance: true,
            threshold_ms: 0,
            include: String::new(),
            exclude: String::new(),
            whole_server: false,
            // A record of the server's is at most 64 K: well under it
            max_sql: 16_000,
            max_argument: 200,
            max_arguments: 100,
        }
    }
}

/// The file of the connected database as the server names it: what a trace
/// configuration is matched against, which before Firebird 5 is never an
/// alias
pub fn trace_database(conn: &Connection) -> Result<String> {
    let rows = conn.query("select mon$database_name from mon$database", vec![])?;
    rows.rows
        .first()
        .and_then(|row| row.first())
        .and_then(|name| name.as_str())
        .map(|name| name.trim_end().to_string())
        .ok_or_else(|| Error::Client { message: "the server did not name the database".into() })
}

/// `path` as a SIMILAR TO pattern that matches it and nothing else
fn literal_pattern(path: &str) -> String {
    let mut pattern = String::with_capacity(path.len() + 8);
    for c in path.chars() {
        if matches!(c, '\\' | '%' | '_' | '|' | '*' | '+' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '^' | '-') {
            pattern.push('\\');
        }
        pattern.push(c);
    }
    pattern
}

/// A filter as one line of a configuration: a line is all a value has
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The configuration text of a session on a server of version `major`.
/// `database` is the file of the one database to follow (see
/// [`trace_database`]); without it, or with `whole_server`, every database.
///
/// The server refuses a configuration naming a parameter it does not know,
/// so each version gets only its own.
pub fn trace_config(options: &TraceOptions, major: u32, database: Option<&str>) -> String {
    let modern = major >= 3;
    let mut lines: Vec<(&str, String)> = vec![("enabled", "true".into())];
    let mut flag = |name: &'static str, on: bool| lines.push((name, on.to_string()));

    flag("log_connections", options.connections);
    flag("log_transactions", options.transactions);
    flag("log_statement_finish", options.statements);
    flag("log_statement_start", options.statements && options.starts);
    flag("log_statement_prepare", options.prepares);
    flag("log_statement_free", options.prepares);
    flag("log_procedure_finish", options.routines);
    flag("log_procedure_start", options.routines && options.starts);
    flag("log_trigger_finish", options.routines);
    flag("log_trigger_start", options.routines && options.starts);
    if modern {
        flag("log_function_finish", options.routines);
        flag("log_function_start", options.routines && options.starts);
    }
    if major >= 5 {
        flag("log_procedure_compile", options.prepares);
        flag("log_function_compile", options.prepares);
        flag("log_trigger_compile", options.prepares);
    }
    flag("log_context", options.context);
    flag("log_errors", options.errors);
    flag("log_warnings", options.errors);
    flag("log_sweep", options.sweep);
    // The session's own start and end, once per attachment: noise
    flag("log_initfini", false);
    flag("print_plan", options.plans);
    if modern {
        flag("explain_plan", options.plans && options.explained_plans);
    }
    flag("print_perf", options.performance);

    lines.push(("time_threshold", options.threshold_ms.to_string()));
    lines.push(("max_sql_length", options.max_sql.clamp(100, 60_000).to_string()));
    lines.push(("max_arg_length", options.max_argument.clamp(10, 10_000).to_string()));
    lines.push(("max_arg_count", options.max_arguments.clamp(1, 1_000).to_string()));
    for (name, filter) in [("include_filter", &options.include), ("exclude_filter", &options.exclude)] {
        let filter = one_line(filter);
        if !filter.is_empty() {
            // Braces open and close a section in the newer syntax
            let filter = if modern { filter.replace('{', "{{").replace('}', "}}") } else { filter };
            lines.push((name, filter));
        }
    }

    let pattern = database.filter(|_| !options.whole_server).map(literal_pattern);
    let mut config = String::new();
    if modern {
        match pattern {
            Some(p) => {
                let _ = writeln!(config, "database = {}", p.replace('{', "{{").replace('}', "}}"));
            }
            None => config.push_str("database\n"),
        }
        config.push_str("{\n");
        for (name, value) in lines {
            let _ = writeln!(config, "\t{name} = {value}");
        }
        config.push_str("}\n");
    } else {
        match pattern {
            Some(p) => {
                let _ = writeln!(config, "<database {p}>");
            }
            None => config.push_str("<database>\n"),
        }
        for (name, value) in lines {
            let _ = writeln!(config, "\t{name} {value}");
        }
        config.push_str("</database>\n");
    }
    config
}

/// How a session ended
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TraceEnd {
    /// The caller stopped it
    Stopped,
    /// The server ended it: no trace plugin is loaded, which the text says,
    /// someone else stopped the session, or the server shut down. A
    /// configuration it cannot read does not end a session: the server
    /// reads it when an attachment comes along, and complains in the text.
    Ended,
}

/// Starts a trace session and hands over what the server writes, as bytes
/// and as they arrive: text in the server's trace format, cut wherever an
/// answer happened to end. The first line names the session ("Trace session
/// ID 7 started"), or says why there is none. An empty piece says the
/// server had nothing to write for a second: what came before it is whole.
///
/// Blocks until the session ends. `stop` is looked at about once a second;
/// the session lives on this service connection, so leaving ends it on the
/// server too.
pub fn run_trace(
    params: &ConnectParams,
    name: &str,
    config: &str,
    stop: &AtomicBool,
    mut on_data: impl FnMut(&[u8]),
) -> Result<TraceEnd> {
    let mut block = Spb::action(ibase::isc_action_svc_trace_start);
    if !name.is_empty() {
        block = block.string(ibase::isc_spb_trc_name, name)?;
    }
    let block = block.string(ibase::isc_spb_trc_cfg, config)?;

    let mut service = attach(params)?;
    service.start(&block.0)?;

    let send = timeout_item();
    loop {
        if stop.load(Ordering::Relaxed) {
            return Ok(TraceEnd::Stopped);
        }
        let answer = parse_answer(&service.query(&send, &[INFO_TO_EOF])?)?;
        if answer.data.is_empty() && !answer.timed_out && !answer.truncated && !answer.not_ready {
            return Ok(TraceEnd::Ended);
        }
        on_data(&answer.data);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn firebird_2_5_gets_the_old_syntax_and_only_its_own_parameters() {
        let config = trace_config(&TraceOptions::default(), 2, Some("/data/sales.fdb"));
        assert!(config.starts_with("<database /data/sales.fdb>\n\tenabled true\n"), "{config}");
        assert!(config.ends_with("</database>\n"), "{config}");
        assert!(config.contains("\tlog_statement_finish true\n"));
        assert!(config.contains("\ttime_threshold 0\n"));
        for unknown in ["log_function", "explain_plan", "_compile", " = "] {
            assert!(!config.contains(unknown), "{unknown} in {config}");
        }
    }

    #[test]
    fn firebird_3_and_later_get_the_new_syntax_and_what_each_added() {
        let options = TraceOptions { prepares: true, explained_plans: true, ..TraceOptions::default() };
        let three = trace_config(&options, 3, Some("/data/sales.fdb"));
        assert!(three.starts_with("database = /data/sales.fdb\n{\n\tenabled = true\n"), "{three}");
        assert!(three.ends_with("}\n"), "{three}");
        assert!(three.contains("\tlog_function_finish = true\n"));
        assert!(three.contains("\texplain_plan = true\n"));
        assert!(!three.contains("_compile"));
        assert_eq!(trace_config(&options, 4, None).contains("_compile"), false);
        let five = trace_config(&options, 5, None);
        assert!(five.starts_with("database\n{\n"), "{five}");
        assert!(five.contains("\tlog_trigger_compile = true\n"));
    }

    #[test]
    fn starts_follow_what_they_are_the_start_of() {
        let config = trace_config(&TraceOptions { routines: false, ..TraceOptions::default() }, 4, None);
        assert!(config.contains("log_statement_start = true"));
        assert!(config.contains("log_procedure_start = false"));
        let config = trace_config(&TraceOptions { starts: false, ..TraceOptions::default() }, 4, None);
        assert!(config.contains("log_statement_start = false"));
        assert!(config.contains("log_statement_finish = true"));
    }

    #[test]
    fn a_database_is_matched_by_its_name_and_nothing_else() {
        assert_eq!(literal_pattern(r"C:\data\my_db (1).fdb"), r"C:\\data\\my\_db \(1\).fdb");
        assert_eq!(literal_pattern("/data/a-b%c.fdb"), r"/data/a\-b\%c.fdb");
        let config = trace_config(&TraceOptions::default(), 4, Some("/data/{x}.fdb"));
        assert!(config.starts_with("database = /data/\\{{x\\}}.fdb\n"), "{config}");
        let all = TraceOptions { whole_server: true, ..TraceOptions::default() };
        assert!(trace_config(&all, 2, Some("/data/sales.fdb")).starts_with("<database>\n"));
    }

    #[test]
    fn a_filter_is_one_line_and_an_empty_one_is_left_out() {
        let options = TraceOptions { include: " %PEDIDO%\n ".into(), ..TraceOptions::default() };
        let config = trace_config(&options, 2, None);
        assert!(config.contains("\tinclude_filter %PEDIDO%\n"), "{config}");
        assert!(!config.contains("exclude_filter"));
    }

    #[test]
    fn lengths_stay_under_what_a_record_holds() {
        let options = TraceOptions { max_sql: 10_000_000, max_arguments: 0, ..TraceOptions::default() };
        let config = trace_config(&options, 5, None);
        assert!(config.contains("max_sql_length = 60000"));
        assert!(config.contains("max_arg_count = 1\n"));
    }
}
