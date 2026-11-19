//! Monitoring sessions: a trace session read on a thread of its own from
//! start to stop, written to a log file as it arrives, and kept in memory
//! as events and as what they add up to, for the activity monitor to ask
//! about.
//!
//! The log is the server's own text, with lines of ours starting with `# `
//! before and after it: what any trace tool reads, and what this one reads
//! back when a recorded session is opened.

use std::collections::{HashMap, VecDeque};
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::{Local, NaiveDateTime};
use fb_core::{Connection, run_trace, trace_config, trace_database, TraceEnd, TraceEvent, TraceItem, TraceOptions, TraceOutcome, TraceReader};
use fb_meta::monitor::{self, Memory};
use fb_sql::lexer::{tokenize, TokenKind};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, State};

use crate::commands::{blocking, AppState};
use crate::error::{AppError, AppResult};

/// The event a session's progress comes through
pub const TICK: &str = "trace-tick";

/// Events kept in memory; older ones are only in the log
const EVENTS_KEPT: usize = 100_000;
/// Seconds of history kept for the charts
const SECONDS_KEPT: usize = 3_600;
/// Different statements told apart; past this, new ones are not counted
const STATEMENTS_KEPT: usize = 20_000;
/// Times kept of a second, to say what most statements stayed under
const TIMES_KEPT: usize = 512;

/// How often the monitoring tables are read for memory. Each read makes
/// the server take a snapshot of what every attachment is doing, which on
/// a busy server is not free.
const SAMPLE_EVERY: Duration = Duration::from_secs(5);
/// The line of a log's head that names the attachment reading them
const MONITOR: &str = "monitor: ATT_";

/// A statement with its literals taken out, so that runs of the same
/// statement with different values count as one
pub fn normalize(sql: &str) -> String {
    let mut out = String::with_capacity(sql.len());
    let mut space = false;
    for token in tokenize(sql) {
        match token.kind {
            TokenKind::Space | TokenKind::LineComment | TokenKind::BlockComment => space = !out.is_empty(),
            kind => {
                if space {
                    out.push(' ');
                }
                space = false;
                match kind {
                    TokenKind::String | TokenKind::Number => out.push('?'),
                    _ => out.push_str(&sql[token.range.clone()]),
                }
            }
        }
    }
    out
}

fn is_routine(kind: &str) -> bool {
    kind.contains("PROCEDURE") || kind.contains("TRIGGER") || kind.contains("FUNCTION")
}

fn sorts(plan: &str) -> bool {
    plan.contains("SORT") || plan.contains("Sort")
}

/// What one second of the session added up to
#[derive(Default)]
struct Second {
    at: i64,
    events: u64,
    statements: u64,
    errors: u64,
    connects: u64,
    ms: u64,
    fetches: u64,
    reads: u64,
    writes: u64,
    marks: u64,
    times: Vec<u32>,
    attachments: Vec<i64>,
}

/// A second as the charts take it
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SecondView {
    /// Seconds since 1970 on the server's clock
    at: i64,
    events: u64,
    statements: u64,
    errors: u64,
    /// Connections made
    connects: u64,
    /// Average time of a statement, in milliseconds
    average: f64,
    /// What 95 in 100 statements stayed under
    high: u32,
    fetches: u64,
    reads: u64,
    writes: u64,
    marks: u64,
    /// Attachments that did something
    attachments: usize,
}

impl Second {
    fn view(&self) -> SecondView {
        let mut times = self.times.clone();
        times.sort_unstable();
        let high = times.get((times.len() * 95).saturating_sub(1) / 100).copied().unwrap_or(0);
        SecondView {
            at: self.at,
            events: self.events,
            statements: self.statements,
            errors: self.errors,
            connects: self.connects,
            average: if self.statements == 0 { 0.0 } else { self.ms as f64 / self.statements as f64 },
            high,
            fetches: self.fetches,
            reads: self.reads,
            writes: self.writes,
            marks: self.marks,
            attachments: self.attachments.len(),
        }
    }
}

/// What the runs of one statement added up to
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatementStats {
    /// The statement, literals replaced by `?`
    sql: String,
    runs: u64,
    ms: u64,
    slowest: u64,
    fetches: u64,
    reads: u64,
    records: u64,
    /// Records read without an index
    natural: u64,
    /// Whether its plan sorts
    sorts: bool,
    plan: String,
    /// The event of its slowest run, while that is still kept
    slowest_event: u64,
    /// The most memory a run of it was seen holding, in bytes; 0 when it
    /// was never caught running
    memory: i64,
}

struct Kept {
    event: TraceEvent,
    /// How deep inside procedures and triggers it ran
    depth: u16,
}

/// Which events a listing wants
#[derive(Debug, Default, Clone, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Filter {
    /// `statements`, `routines`, `transactions`, `connections`, `errors`,
    /// or anything else for all
    group: String,
    attachment: Option<i64>,
    /// Only what took at least this long
    min_ms: u64,
    /// Only what has this in its statement, object, parameters or message
    text: String,
}

impl Filter {
    fn takes(&self, event: &TraceEvent, needle: &str) -> bool {
        let kind = event.kind.as_str();
        let failed = event.outcome != TraceOutcome::Ok || kind == "ERROR" || kind == "WARNING";
        let group = match self.group.as_str() {
            "statements" => kind.contains("STATEMENT"),
            "routines" => is_routine(kind),
            "transactions" => kind.contains("TRANSACTION") || kind.contains("RETAINING"),
            "connections" => kind.ends_with("_DATABASE"),
            "errors" => failed,
            _ => true,
        };
        group
            && self.attachment.is_none_or(|a| event.attachment == Some(a))
            && (self.min_ms == 0 || event.ms.is_some_and(|ms| ms >= self.min_ms))
            && (needle.is_empty()
                || [&event.sql, &event.object, &event.message].iter().any(|t| t.to_lowercase().contains(needle))
                || event.params.iter().any(|p| p.to_lowercase().contains(needle)))
    }
}

/// An event as a line of the list
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Row {
    /// Its number in the session, from 0
    seq: u64,
    at: String,
    kind: String,
    failed: bool,
    attachment: Option<i64>,
    depth: u16,
    what: String,
    ms: Option<u64>,
    fetches: u64,
}

/// What an event is about, in a line
fn what(event: &TraceEvent) -> String {
    let text = if !event.sql.is_empty() {
        event.sql.split_whitespace().collect::<Vec<_>>().join(" ")
    } else if !event.object.is_empty() && !event.message.is_empty() {
        format!("{} · {}", event.message.replace('\n', " "), event.object)
    } else if !event.object.is_empty() {
        event.object.clone()
    } else if !event.message.is_empty() {
        event.message.replace('\n', " ")
    } else if let Some(transaction) = event.transaction {
        format!("TRA_{transaction} · {}", event.transaction_options)
    } else if event.kind.ends_with("_DATABASE") {
        format!("{} · {} · {}", event.user, event.address, event.process)
    } else {
        String::new()
    };
    match text.char_indices().nth(400) {
        Some((cut, _)) => format!("{}…", &text[..cut]),
        None => text,
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Page {
    /// Events the filter takes, of those kept
    matching: u64,
    rows: Vec<Row>,
}

/// An event in full, with what the runs of its statement add up to
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Detail {
    seq: u64,
    depth: u16,
    event: TraceEvent,
    statement: Option<StatementStats>,
}

/// A session's events and what they add up to
#[derive(Default)]
pub struct Store {
    events: VecDeque<Kept>,
    /// The number of the oldest event kept
    first: u64,
    total: u64,
    gaps: u64,
    notices: Vec<String>,
    seconds: VecDeque<Second>,
    statements: HashMap<String, StatementStats>,
    /// How deep each attachment is inside routines
    depths: HashMap<i64, u16>,
    /// The attachment that reads the monitoring tables for this session:
    /// what it runs is not what is being looked for
    ignored: Option<i64>,
    /// Where the memory was when last looked at
    memory: Option<Memory>,
    /// The most memory each statement was seen holding
    peaks: HashMap<String, i64>,
    /// Connections made, by the program and the machine that made them
    connectors: HashMap<String, u64>,
}

/// Who connects, as a few words: the program's file and the machine,
/// without what changes at every connection (the process id, the port)
fn connector(event: &TraceEvent) -> String {
    let program = event.process.rsplit_once(':').map_or(event.process.as_str(), |(path, _)| path);
    let program = program.rsplit(['/', '\\']).next().unwrap_or_default();
    let machine = event.address.split_once(':').map_or(event.address.as_str(), |(_, rest)| rest);
    let machine = machine.rsplit_once('/').map_or(machine, |(host, _)| host);
    [program, machine].iter().filter(|part| !part.is_empty()).copied().collect::<Vec<_>>().join(" · ")
}

impl Store {
    pub fn take(&mut self, item: TraceItem) {
        match item {
            TraceItem::Gap => self.gaps += 1,
            TraceItem::Notice(notice) => {
                if !notice.starts_with('#') && self.notices.len() < 100 {
                    self.notices.push(notice);
                }
            }
            TraceItem::Event(event) => self.event(*event),
        }
    }

    /// What the monitoring tables said about memory just now
    pub fn sample(&mut self, mut memory: Memory) {
        // The connection that reads them is not one of the database's users
        memory.attachments.retain(|a| Some(a.id) != self.ignored);
        for statement in &memory.statements {
            if let Some(sql) = &statement.sql {
                let peak = self.peaks.entry(normalize(sql)).or_default();
                *peak = (*peak).max(statement.peak).max(statement.used);
            }
        }
        if self.peaks.len() > STATEMENTS_KEPT {
            self.peaks.clear();
        }
        self.memory = Some(memory);
    }

    fn with_memory(&self, mut stats: StatementStats) -> StatementStats {
        stats.memory = self.peaks.get(&stats.sql).copied().unwrap_or(0);
        stats
    }

    fn event(&mut self, event: TraceEvent) {
        if self.ignored.is_some() && event.attachment == self.ignored {
            return;
        }
        let kind = event.kind.as_str();
        let routine = is_routine(kind);
        let depth = match event.attachment {
            Some(attachment) => {
                let depth = self.depths.entry(attachment).or_default();
                if kind == "DETACH_DATABASE" {
                    *depth = 0;
                }
                if routine && kind.ends_with("_FINISH") {
                    *depth = depth.saturating_sub(1);
                }
                let at = *depth;
                if routine && kind.ends_with("_START") {
                    *depth = (*depth + 1).min(64);
                }
                at
            }
            None => 0,
        };
        let seq = self.total;
        let failed = event.outcome != TraceOutcome::Ok || kind == "ERROR";
        let finished = kind == "EXECUTE_STATEMENT_FINISH";
        let connected = kind == "ATTACH_DATABASE" && !failed;
        if connected && (self.connectors.len() < 1_000 || self.connectors.contains_key(&connector(&event))) {
            *self.connectors.entry(connector(&event)).or_default() += 1;
        }

        if let Some(second) = self.second(&event.at) {
            second.events += 1;
            second.errors += u64::from(failed);
            second.connects += u64::from(connected);
            if let Some(attachment) = event.attachment {
                if !second.attachments.contains(&attachment) {
                    second.attachments.push(attachment);
                }
            }
            if finished {
                let ms = event.ms.unwrap_or(0);
                second.statements += 1;
                second.ms += ms;
                if second.times.len() < TIMES_KEPT {
                    second.times.push(ms.min(u64::from(u32::MAX)) as u32);
                }
                // What a statement inside a routine cost is in what the
                // statement that called the routine cost
                if depth == 0 {
                    second.fetches += event.fetches;
                    second.reads += event.reads;
                    second.writes += event.writes;
                    second.marks += event.marks;
                }
            }
        }

        if finished && !event.sql.is_empty() {
            let key = normalize(&event.sql);
            if self.statements.len() < STATEMENTS_KEPT || self.statements.contains_key(&key) {
                let stats = self.statements.entry(key.clone()).or_insert_with(|| StatementStats { sql: key, ..Default::default() });
                let ms = event.ms.unwrap_or(0);
                stats.runs += 1;
                stats.ms += ms;
                stats.fetches += event.fetches;
                stats.reads += event.reads;
                stats.records += event.records.unwrap_or(0);
                stats.natural += event.tables.iter().map(|t| t.natural).sum::<u64>();
                if ms >= stats.slowest {
                    stats.slowest = ms;
                    stats.slowest_event = seq;
                }
                if !event.plan.is_empty() {
                    stats.sorts = sorts(&event.plan);
                    stats.plan.clone_from(&event.plan);
                }
            }
        }

        self.total += 1;
        self.events.push_back(Kept { event, depth });
        if self.events.len() > EVENTS_KEPT {
            self.events.pop_front();
            self.first += 1;
        }
    }

    /// The second an event belongs to. The server's clock is trusted to go
    /// forward; an event from before the last second counts in the last.
    fn second(&mut self, at: &str) -> Option<&mut Second> {
        let at = NaiveDateTime::parse_from_str(at, "%Y-%m-%dT%H:%M:%S%.f").ok()?.and_utc().timestamp();
        if self.seconds.back().is_none_or(|last| last.at < at) {
            self.seconds.push_back(Second { at, ..Default::default() });
            if self.seconds.len() > SECONDS_KEPT {
                self.seconds.pop_front();
            }
        }
        self.seconds.back_mut()
    }

    /// The last `count` seconds, oldest first
    pub fn series(&self, count: usize) -> Vec<SecondView> {
        self.seconds.iter().skip(self.seconds.len().saturating_sub(count)).map(Second::view).collect()
    }

    pub fn page(&self, filter: &Filter, offset: u64, limit: usize) -> Page {
        let needle = filter.text.trim().to_lowercase();
        let mut page = Page { matching: 0, rows: Vec::new() };
        for (i, kept) in self.events.iter().enumerate() {
            if !filter.takes(&kept.event, &needle) {
                continue;
            }
            if page.matching >= offset && page.rows.len() < limit {
                let event = &kept.event;
                page.rows.push(Row {
                    seq: self.first + i as u64,
                    at: event.at.clone(),
                    kind: event.kind.clone(),
                    failed: event.outcome != TraceOutcome::Ok || event.kind == "ERROR",
                    attachment: event.attachment,
                    depth: kept.depth,
                    what: what(event),
                    ms: event.ms,
                    fetches: event.fetches,
                });
            }
            page.matching += 1;
        }
        page
    }

    pub fn detail(&self, seq: u64) -> Option<Detail> {
        let kept = self.events.get(usize::try_from(seq.checked_sub(self.first)?).ok()?)?;
        let statement = (!kept.event.sql.is_empty())
            .then(|| self.statements.get(&normalize(&kept.event.sql)).cloned())
            .flatten()
            .map(|stats| self.with_memory(stats));
        Some(Detail { seq, depth: kept.depth, event: kept.event.clone(), statement })
    }

    /// The statements that cost the most by `by`: `runs`, `fetches`,
    /// `natural`, `slowest`, `records`, `memory`, or the total time
    pub fn top(&self, by: &str, sorting_only: bool, limit: usize) -> Vec<StatementStats> {
        let key = |s: &StatementStats| match by {
            "memory" => self.peaks.get(&s.sql).copied().unwrap_or(0).max(0) as u64,
            "runs" => s.runs,
            "fetches" => s.fetches,
            "natural" => s.natural,
            "slowest" => s.slowest,
            "records" => s.records,
            _ => s.ms,
        };
        let mut top: Vec<&StatementStats> = self.statements.values().filter(|s| !sorting_only || s.sorts).collect();
        top.sort_by(|a, b| key(b).cmp(&key(a)).then_with(|| b.ms.cmp(&a.ms)).then_with(|| a.sql.cmp(&b.sql)));
        top.into_iter().take(limit).map(|s| self.with_memory(s.clone())).collect()
    }
}

/// Where a session stands
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Standing {
    Recording,
    /// Stopped here
    Stopped,
    /// Ended by the server
    Ended,
    Failed,
    /// A recorded session, opened from its log
    Opened,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Tick {
    run: u64,
    standing: Standing,
    error: Option<String>,
    /// Events so far, and the number of the oldest still kept
    total: u64,
    first: u64,
    /// Times the server dropped events for want of room
    gaps: u64,
    notices: Vec<String>,
    /// Who made the most connections, and how many: a program that
    /// connects for every statement stands out here
    connector: Option<(String, u64)>,
    /// The latest seconds: the last may still grow
    seconds: Vec<SecondView>,
    log: String,
}

struct Trace {
    /// The registration it follows: the one it was started on, or the one
    /// whose folder a log opened from disk is in
    registration: Option<String>,
    stop: AtomicBool,
    store: Mutex<Store>,
    standing: Mutex<(Standing, Option<String>)>,
    log: PathBuf,
}

impl Trace {
    fn tick(&self, run: u64, seconds: usize) -> Tick {
        let store = self.store.lock().unwrap();
        let (standing, error) = self.standing.lock().unwrap().clone();
        Tick {
            run,
            standing,
            error,
            total: store.total,
            first: store.first,
            gaps: store.gaps,
            notices: store.notices.clone(),
            connector: store.connectors.iter().max_by_key(|(name, count)| (**count, std::cmp::Reverse(*name))).map(|(n, c)| (n.clone(), *c)),
            seconds: store.series(seconds),
            log: self.log.display().to_string(),
        }
    }
}

/// The sessions open, by the id the UI gave each, and where their logs go
pub struct Traces {
    dir: PathBuf,
    open: Mutex<HashMap<u64, Arc<Trace>>>,
}

impl Traces {
    pub fn new(dir: &Path) -> Self {
        Self { dir: dir.join("traces"), open: Mutex::default() }
    }

    fn get(&self, run: u64) -> AppResult<Arc<Trace>> {
        self.open
            .lock()
            .unwrap()
            .get(&run)
            .cloned()
            .ok_or_else(|| AppError::NotFound { message: "This monitoring session is no longer open.".into() })
    }

    /// What the newest session open on `registration` adds up to, for the
    /// agent: its statements without their literals, and nothing of who
    /// connected from where
    pub(crate) fn summary(&self, registration: &str, by: &str, limit: usize) -> Option<TraceSummary> {
        let (run, trace) = self
            .open
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, t)| t.registration.as_deref() == Some(registration))
            .max_by_key(|(run, _)| **run)
            .map(|(run, t)| (*run, t.clone()))?;
        let (standing, error) = trace.standing.lock().unwrap().clone();
        let store = trace.store.lock().unwrap();
        let seconds = store.seconds.len();
        let statements: u64 = store.seconds.iter().map(|s| s.statements).sum();
        let ms: u64 = store.seconds.iter().map(|s| s.ms).sum();
        Some(TraceSummary {
            session: run,
            standing,
            error,
            events: store.total,
            gaps: store.gaps,
            seconds,
            statements,
            errors: store.seconds.iter().map(|s| s.errors).sum(),
            connects: store.seconds.iter().map(|s| s.connects).sum(),
            average_ms: if statements == 0 { 0.0 } else { ms as f64 / statements as f64 },
            worst_second_p95_ms: store.seconds.iter().map(|s| s.view().high).max().unwrap_or(0),
            memory_used: store.memory.as_ref().map(|m| m.used),
            memory_peak: store.memory.as_ref().map(|m| m.peak),
            top: store.top(by, false, limit),
        })
    }
}

/// A monitoring session as the agent is told of it
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TraceSummary {
    session: u64,
    standing: Standing,
    error: Option<String>,
    /// Events in all, and the times the server dropped some
    events: u64,
    gaps: u64,
    /// The seconds the figures below are over: the latest ones kept
    seconds: usize,
    statements: u64,
    errors: u64,
    connects: u64,
    average_ms: f64,
    worst_second_p95_ms: u32,
    memory_used: Option<i64>,
    memory_peak: Option<i64>,
    /// The statements that cost the most, literals replaced by `?`
    top: Vec<StatementStats>,
}

/// A log's lines of ours: each starts with `# `
fn remarks(file: &mut impl Write, lines: &str) -> std::io::Result<()> {
    for line in lines.lines() {
        writeln!(file, "# {line}")?;
    }
    Ok(())
}

/// Starts monitoring a connected registration's server. Returns once the
/// log is open; what follows comes through [`TICK`] events named by `run`.
#[tauri::command]
pub async fn trace_start(app: AppHandle, id: String, run: u64, options: TraceOptions) -> AppResult<Tick> {
    let events = app.clone();
    blocking(app, move |state| {
        let params = state.params(&id)?;
        let session = state.session(&id)?;
        let info = session.info().clone();
        let database = trace_database(&session)?;
        let config = trace_config(&options, info.major, Some(&database));

        // The monitoring tables are read on a connection of this session's
        // own, so that a query running in an editor does not hold them up.
        // Without one, the session goes on without memory figures.
        let sampler = Connection::open(&params).ok().and_then(|conn| {
            let own = conn.query("select current_connection from rdb$database", vec![]).ok()?;
            Some((conn, own.rows.first()?.first()?.as_i64()?))
        });
        let ignored = sampler.as_ref().map(|(_, id)| *id);

        let dir = state.traces.dir.join(&id);
        fs::create_dir_all(&dir).map_err(|e| AppError::storage(&dir, e))?;
        let now = Local::now();
        let log = dir.join(format!("{}.log", now.format("%Y-%m-%d_%H%M%S")));
        let mut file = BufWriter::new(File::create(&log).map_err(|e| AppError::storage(&log, e))?);
        let head = format!(
            "N Studio monitoring session\nstarted: {}\nserver: Firebird {}\ndatabase: {database}\nuser: {}{}\n\n{config}",
            now.format("%Y-%m-%d %H:%M:%S"),
            info.engine_version,
            params.user,
            ignored.map(|id| format!("\n{MONITOR}{id}")).unwrap_or_default(),
        );
        remarks(&mut file, &head).and_then(|()| file.flush()).map_err(|e| AppError::storage(&log, e))?;

        let trace = Arc::new(Trace {
            registration: Some(id.clone()),
            stop: AtomicBool::new(false),
            store: Mutex::new(Store { ignored, ..Store::default() }),
            standing: Mutex::new((Standing::Recording, None)),
            log: log.clone(),
        });
        state.traces.open.lock().unwrap().insert(run, trace.clone());
        let first = trace.tick(run, 0);

        if let Some((conn, _)) = sampler {
            let trace = trace.clone();
            let _ = std::thread::Builder::new().name("trace-memory".into()).spawn(move || {
                let recording = || trace.standing.lock().unwrap().0 == Standing::Recording && !trace.stop.load(Ordering::Relaxed);
                while recording() {
                    match monitor::memory(&conn) {
                        Ok(memory) => trace.store.lock().unwrap().sample(memory),
                        // Not allowed to, or the connection went: no figures
                        Err(_) => break,
                    }
                    let waited = Instant::now();
                    while recording() && waited.elapsed() < SAMPLE_EVERY {
                        std::thread::sleep(Duration::from_millis(200));
                    }
                }
            });
        }

        std::thread::Builder::new()
            .name("trace".into())
            .spawn(move || {
                let mut reader = TraceReader::new();
                let mut sent = Instant::now();
                let mut written = Ok(());
                let end = run_trace(&params, "N Studio", &config, &trace.stop, |piece| {
                    if written.is_ok() {
                        written = if piece.is_empty() { file.flush() } else { file.write_all(piece) };
                    }
                    {
                        let mut store = trace.store.lock().unwrap();
                        let mut take = |item: TraceItem, _: &str| store.take(item);
                        if piece.is_empty() {
                            reader.pause(&mut take);
                        } else {
                            reader.feed(piece, &mut take);
                        }
                    }
                    if piece.is_empty() || sent.elapsed() >= Duration::from_millis(250) {
                        let _ = events.emit(TICK, trace.tick(run, 3));
                        sent = Instant::now();
                    }
                });
                let (total, gaps) = {
                    let mut store = trace.store.lock().unwrap();
                    reader.finish(&mut |item, _| store.take(item));
                    (store.total, store.gaps)
                };
                let standing = match (&end, &written) {
                    (Err(e), _) => (Standing::Failed, Some(AppError::from(e.clone()).to_string())),
                    (_, Err(e)) => (Standing::Failed, Some(format!("{}: {e}", trace.log.display()))),
                    (Ok(TraceEnd::Stopped), _) => (Standing::Stopped, None),
                    (Ok(TraceEnd::Ended), _) => (Standing::Ended, None),
                };
                let foot = format!(
                    "\nended: {}\nevents: {total}\ngaps: {gaps}{}",
                    Local::now().format("%Y-%m-%d %H:%M:%S"),
                    standing.1.as_deref().map(|e| format!("\nerror: {e}")).unwrap_or_default(),
                );
                let _ = remarks(&mut file, &foot).and_then(|()| file.flush());
                *trace.standing.lock().unwrap() = standing;
                let _ = events.emit(TICK, trace.tick(run, 3));
            })
            .map_err(|e| AppError::Connection { message: format!("The monitoring thread did not start: {e}") })?;
        Ok(first)
    })
    .await
}

/// Stops a session. It stays open to be looked at until [`trace_close`].
#[tauri::command]
pub fn trace_stop(state: State<'_, AppState>, run: u64) -> bool {
    match state.traces.open.lock().unwrap().get(&run) {
        Some(trace) => {
            trace.stop.store(true, Ordering::Relaxed);
            true
        }
        None => false,
    }
}

/// Stops a session if it runs and forgets what it kept. Its log stays.
#[tauri::command]
pub fn trace_close(state: State<'_, AppState>, run: u64) {
    if let Some(trace) = state.traces.open.lock().unwrap().remove(&run) {
        trace.stop.store(true, Ordering::Relaxed);
    }
}

/// Where a session stands, with all the seconds kept: what a view asks
/// when it opens
#[tauri::command]
pub fn trace_state(state: State<'_, AppState>, run: u64) -> AppResult<Tick> {
    Ok(state.traces.get(run)?.tick(run, SECONDS_KEPT))
}

#[tauri::command]
pub async fn trace_page(app: AppHandle, run: u64, filter: Filter, offset: u64, limit: usize) -> AppResult<Page> {
    blocking(app, move |state| Ok(state.traces.get(run)?.store.lock().unwrap().page(&filter, offset, limit.min(1_000)))).await
}

#[tauri::command]
pub fn trace_event(state: State<'_, AppState>, run: u64, seq: u64) -> AppResult<Detail> {
    state
        .traces
        .get(run)?
        .store
        .lock()
        .unwrap()
        .detail(seq)
        .ok_or_else(|| AppError::NotFound { message: "This event is no longer kept in memory: it is in the log.".into() })
}

#[tauri::command]
pub async fn trace_top(app: AppHandle, run: u64, by: String, sorting_only: bool, limit: usize) -> AppResult<Vec<StatementStats>> {
    blocking(app, move |state| Ok(state.traces.get(run)?.store.lock().unwrap().top(&by, sorting_only, limit.min(1_000)))).await
}

/// Where the database's memory was when last looked at; nothing for a
/// session that could not look, or one opened from its log
#[tauri::command]
pub fn trace_memory(state: State<'_, AppState>, run: u64) -> AppResult<Option<Memory>> {
    Ok(state.traces.get(run)?.store.lock().unwrap().memory.clone())
}

/// A recorded session
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Recorded {
    path: String,
    name: String,
    bytes: u64,
}

/// The logs of a registration's sessions, newest first
#[tauri::command]
pub async fn trace_sessions(app: AppHandle, id: String) -> AppResult<Vec<Recorded>> {
    blocking(app, move |state| {
        let dir = state.traces.dir.join(&id);
        let mut logs = Vec::new();
        let Ok(entries) = fs::read_dir(&dir) else { return Ok(logs) };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "log") {
                logs.push(Recorded {
                    name: path.file_stem().unwrap_or_default().to_string_lossy().into_owned(),
                    bytes: entry.metadata().map(|m| m.len()).unwrap_or(0),
                    path: path.display().to_string(),
                });
            }
        }
        logs.sort_by(|a, b| b.name.cmp(&a.name));
        Ok(logs)
    })
    .await
}

/// Shows the folder a registration's logs are kept in, in the system's
/// file manager; made first if nothing was recorded yet
#[tauri::command]
pub async fn trace_folder(app: AppHandle, id: String) -> AppResult<()> {
    blocking(app, move |state| {
        let dir = state.traces.dir.join(&id);
        let shown = fs::create_dir_all(&dir).and_then(|()| {
            let opener = if cfg!(target_os = "macos") {
                "open"
            } else if cfg!(target_os = "windows") {
                "explorer"
            } else {
                "xdg-open"
            };
            std::process::Command::new(opener).arg(&dir).spawn().map(drop)
        });
        shown.map_err(|e| AppError::storage(&dir, e))
    })
    .await
}

/// What a log holds, read back
fn read_log(bytes: &[u8]) -> Store {
    let lines = || bytes.split_inclusive(|&b| b == b'\n');
    let ignored = lines()
        .filter_map(|line| std::str::from_utf8(line.strip_prefix(b"# ")?).ok()?.trim().strip_prefix(MONITOR)?.parse().ok())
        .next();
    let mut store = Store { ignored, ..Store::default() };
    let mut reader = TraceReader::new();
    let mut take = |item: TraceItem, _: &str| store.take(item);
    // Our own lines are not the server's: one after the last record would
    // read as part of it
    for line in lines().filter(|line| !line.starts_with(b"#")) {
        reader.feed(line, &mut take);
    }
    reader.finish(&mut take);
    store
}

/// Opens a recorded session's log as the session `run`, to be asked about
/// like one that is recording
#[tauri::command]
pub async fn trace_open(app: AppHandle, run: u64, path: PathBuf) -> AppResult<Tick> {
    blocking(app, move |state| {
        let bytes = fs::read(&path).map_err(|e| AppError::storage(&path, e))?;
        let registration = path
            .parent()
            .filter(|dir| dir.parent() == Some(state.traces.dir.as_path()))
            .and_then(|dir| dir.file_name())
            .map(|name| name.to_string_lossy().into_owned());
        let trace = Arc::new(Trace {
            registration,
            stop: AtomicBool::new(true),
            store: Mutex::new(read_log(&bytes)),
            standing: Mutex::new((Standing::Opened, None)),
            log: path,
        });
        let tick = trace.tick(run, SECONDS_KEPT);
        state.traces.open.lock().unwrap().insert(run, trace);
        Ok(tick)
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn statement(at: &str, attachment: i64, sql: &str, ms: u64, fetches: u64, plan: &str) -> String {
        format!(
            "2024-05-01T10:15:{at} (1:0x1) EXECUTE_STATEMENT_FINISH\n\
             \t/data/sales.fdb (ATT_{attachment}, APP:NONE, UTF8, TCPv4:10.0.0.5/53122)\n\
             \t\t(TRA_88, CONCURRENCY | WAIT | READ_WRITE)\n\nStatement 5:\n{}\n{sql}\n{}\n{plan}\n\n\
             1 records fetched\n{ms:>7} ms, {fetches} fetch(es)\n\n",
            "-".repeat(79),
            "^".repeat(79),
        )
    }

    fn routine(at: &str, kind: &str, name: &str) -> String {
        format!(
            "2024-05-01T10:15:{at} (1:0x1) EXECUTE_PROCEDURE_{kind}\n\
             \t/data/sales.fdb (ATT_17, APP:NONE, UTF8, TCPv4:10.0.0.5/53122)\n\nProcedure {name}:\n      2 ms\n\n"
        )
    }

    fn store(text: &str) -> Store {
        read_log(text.as_bytes())
    }

    #[test]
    fn runs_of_a_statement_with_other_values_are_one_statement() {
        assert_eq!(normalize("select  *\n from T -- all\n where A = 12 and B='x''y' and C = :c"), "select * from T where A = ? and B=? and C = :c");
        assert_eq!(normalize("select \"A b\" from t1 where x = 1.5e3"), "select \"A b\" from t1 where x = ?");

        let text = [
            statement("40.0001", 17, "select * from T where ID = 1", 10, 100, "PLAN (T NATURAL)"),
            statement("40.5000", 17, "select * from T where ID = 2", 30, 300, "PLAN (T NATURAL)"),
            statement("41.0000", 18, "select A from U order by A", 5, 50, "PLAN SORT (U NATURAL)"),
        ]
        .concat();
        let store = store(&text);
        let top = store.top("time", false, 10);
        assert_eq!(top.len(), 2);
        assert_eq!((top[0].sql.as_str(), top[0].runs, top[0].ms, top[0].slowest, top[0].fetches), ("select * from T where ID = ?", 2, 40, 30, 400));
        assert_eq!((top[0].slowest_event, top[0].sorts), (1, false));
        let sorting = store.top("time", true, 10);
        assert_eq!(sorting.len(), 1);
        assert!(sorting[0].sorts && sorting[0].sql.contains("order by"));
        assert_eq!(store.top("runs", false, 1)[0].runs, 2);
    }

    #[test]
    fn seconds_add_up_what_ran_in_them() {
        let text = [
            statement("40.0001", 17, "select 1 from T", 10, 100, ""),
            statement("40.5000", 18, "select 2 from T", 30, 300, ""),
            statement("42.0000", 17, "select 3 from T", 4, 40, ""),
        ]
        .concat();
        let series = store(&text).series(10);
        assert_eq!(series.len(), 2);
        assert_eq!((series[0].statements, series[0].fetches, series[0].attachments), (2, 400, 2));
        assert_eq!((series[0].average, series[0].high), (20.0, 30));
        assert_eq!(series[1].at - series[0].at, 2);
        assert_eq!(store(&text).series(1).len(), 1);
    }

    #[test]
    fn what_runs_inside_a_routine_is_nested_and_counted_once() {
        let text = [
            routine("40.0001", "START", "P_OUTER"),
            statement("40.0002", 17, "select 1 from T", 1, 10, ""),
            routine("40.0003", "START", "P_INNER"),
            statement("40.0004", 17, "select 2 from T", 1, 20, ""),
            routine("40.0005", "FINISH", "P_INNER"),
            routine("40.0006", "FINISH", "P_OUTER"),
            statement("40.0007", 17, "execute procedure P_OUTER", 3, 35, ""),
        ]
        .concat();
        let store = store(&text);
        let page = store.page(&Filter::default(), 0, 100);
        assert_eq!(page.rows.iter().map(|r| r.depth).collect::<Vec<_>>(), [0, 1, 1, 2, 1, 0, 0]);
        assert_eq!(page.rows[0].what, "P_OUTER");
        // Only the outer statement's pages: the inner ones are in them
        assert_eq!(store.series(1)[0].fetches, 35);
        assert_eq!(store.series(1)[0].statements, 3);
    }

    #[test]
    fn a_listing_is_filtered_and_paged() {
        let text = [
            statement("40.0001", 17, "select 1 from PEDIDO", 10, 1, ""),
            routine("40.0002", "FINISH", "P_TOTAL"),
            statement("40.0003", 18, "select 2 from CLIENTE", 200, 1, ""),
            statement("40.0004", 17, "select 3 from PEDIDO", 300, 1, ""),
        ]
        .concat();
        let store = store(&text);
        let take = |filter: Filter| store.page(&filter, 0, 100).rows.iter().map(|r| r.seq).collect::<Vec<_>>();
        assert_eq!(take(Filter { group: "statements".into(), ..Default::default() }), [0, 2, 3]);
        assert_eq!(take(Filter { group: "routines".into(), ..Default::default() }), [1]);
        assert_eq!(take(Filter { attachment: Some(18), ..Default::default() }), [2]);
        assert_eq!(take(Filter { min_ms: 100, ..Default::default() }), [2, 3]);
        assert_eq!(take(Filter { text: " pedido ".into(), ..Default::default() }), [0, 3]);
        let page = store.page(&Filter::default(), 1, 2);
        assert_eq!((page.matching, page.rows.iter().map(|r| r.seq).collect::<Vec<_>>()), (4, vec![1, 2]));
        let detail = store.detail(3).unwrap();
        // It ran twice, with another number
        assert_eq!((detail.event.sql.as_str(), detail.statement.unwrap().runs), ("select 3 from PEDIDO", 2));
        assert!(store.detail(9).is_none());
    }

    #[test]
    fn connections_are_counted_by_who_makes_them() {
        let attach = |at: &str, id: i64, port: u32| {
            format!(
                "2024-05-01T10:15:{at} (1:0x1) ATTACH_DATABASE\n\t/data/sales.fdb (ATT_{id}, APP:NONE, UTF8, TCPv4:10.0.0.5/{port})\n\tC:\\apps\\api\\node.exe:{port}\n\n"
            )
        };
        let failed = attach("41.0000", 9, 3).replace(") ATTACH", ") FAILED ATTACH");
        let store = store(&[attach("40.0001", 1, 5001), attach("40.5000", 2, 5002), failed].concat());
        assert_eq!(store.connectors.get("node.exe · 10.0.0.5"), Some(&2));
        assert_eq!(store.connectors.len(), 1);
        assert_eq!(store.series(10).iter().map(|s| s.connects).collect::<Vec<_>>(), [2, 0]);
    }

    #[test]
    fn what_the_monitor_itself_runs_is_left_out_and_memory_is_kept_by_statement() {
        let text = [
            statement("40.0001", 17, "select 1 from T", 10, 1, "PLAN SORT (T NATURAL)"),
            statement("40.0002", 99, "select mon$memory_used from mon$memory_usage", 10, 1, ""),
        ]
        .concat();
        let mut log = Vec::new();
        remarks(&mut log, &format!("user: X\n{MONITOR}99")).unwrap();
        log.extend_from_slice(text.as_bytes());
        let mut store = read_log(&log);
        assert_eq!(store.total, 1);
        assert_eq!(store.top("time", false, 10).len(), 1);

        let running = |sql: &str, used, peak| monitor::MemoryUse {
            id: 1,
            attachment: 17,
            user: None,
            address: None,
            sql: Some(sql.into()),
            used,
            peak,
        };
        store.sample(Memory { used: 10, peak: 20, attachments: vec![], statements: vec![running("select 5 from T", 300, 900)] });
        store.sample(Memory { used: 10, peak: 20, attachments: vec![], statements: vec![running("select 6 from T", 400, 500)] });
        assert_eq!(store.top("memory", true, 10)[0].memory, 900);
        assert_eq!(store.detail(0).unwrap().statement.unwrap().memory, 900);
        assert_eq!(store.memory.as_ref().unwrap().statements[0].used, 400);
    }

    #[test]
    fn a_log_reads_back_without_the_lines_that_are_ours() {
        let mut log = Vec::new();
        remarks(&mut log, "N Studio monitoring session\n\ndatabase\n{\n}").unwrap();
        log.extend_from_slice(b"Trace session ID 7 started\n");
        log.extend_from_slice(statement("40.0001", 17, "select 1 from T", 10, 1, "PLAN (T NATURAL)").as_bytes());
        log.extend_from_slice(b"--- Session 7 is suspended as its log is full ---\n");
        remarks(&mut log, "\nended: now\nevents: 1").unwrap();
        let store = read_log(&log);
        assert_eq!((store.total, store.gaps), (1, 1));
        assert_eq!(store.notices, ["Trace session ID 7 started"]);
        assert_eq!(store.detail(0).unwrap().event.plan, "PLAN (T NATURAL)");
    }
}
