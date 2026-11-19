//! Reads the text of a trace session back into events.
//!
//! The text is what the server's trace plugin prints (`TracePluginImpl.cpp`
//! in Firebird's source): a header line with the time and the event's name,
//! then the attachment, the transaction, and whatever the event has to say,
//! and an empty line. It was made for people to read and has no grammar, so
//! this goes line by line, takes what it knows and keeps the rest: every
//! record is also handed over as the text it came in, and one that is not
//! understood is still an event with its time and its name.

use serde::Serialize;

use crate::service::text;

/// Whether what the event reports worked
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TraceOutcome {
    #[default]
    Ok,
    Failed,
    Unauthorized,
}

/// Records one table had read and written, as a statement's counters list
/// them
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct TableCounts {
    pub name: String,
    /// Records read in storage order: without an index
    pub natural: u64,
    pub index: u64,
    pub update: u64,
    pub insert: u64,
    pub delete: u64,
    pub backout: u64,
    pub purge: u64,
    pub expunge: u64,
}

/// One record of a trace
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TraceEvent {
    /// The server's clock, as written: `2024-05-01T10:15:42.1234`
    pub at: String,
    /// The event's name without what says it failed, such as
    /// `EXECUTE_STATEMENT_FINISH`. `ERROR` and `WARNING` for those, with
    /// where they happened in `object`.
    pub kind: String,
    pub outcome: TraceOutcome,
    pub database: String,
    pub attachment: Option<i64>,
    pub user: String,
    pub role: String,
    pub charset: String,
    /// `TCPv4:10.0.0.5/53122`, or empty for a connection made inside the
    /// server
    pub address: String,
    /// The client program and its process id, when the server knows them
    pub process: String,
    pub transaction: Option<i64>,
    /// `CONCURRENCY | WAIT | READ_WRITE`
    pub transaction_options: String,
    pub statement: Option<i64>,
    /// The procedure, function or trigger of a routine's event
    pub object: String,
    pub sql: String,
    pub plan: String,
    /// `param0 = integer, "2"`, a line each
    pub params: Vec<String>,
    pub records: Option<u64>,
    pub ms: Option<u64>,
    pub reads: u64,
    pub writes: u64,
    pub fetches: u64,
    pub marks: u64,
    pub tables: Vec<TableCounts>,
    /// What an error or a warning says
    pub message: String,
}

/// What a trace's text is made of
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TraceItem {
    Event(Box<TraceEvent>),
    /// Something the session said outside any record: that it started and
    /// under which id, or why it did not
    Notice(String),
    /// The server's buffer filled and events were dropped from here on,
    /// until it was read enough to go on
    Gap,
}

/// `2024-05-01T10:15:42.1234 (` at the start of a line: a record begins
fn is_header(line: &str) -> bool {
    const SHAPE: &[u8] = b"dddd-dd-ddTdd:dd:dd.dddd (";
    let bytes = line.as_bytes();
    bytes.len() > SHAPE.len()
        && SHAPE.iter().zip(bytes).all(|(shape, b)| if *shape == b'd' { b.is_ascii_digit() } else { shape == b })
}

fn is_gap(line: &str) -> bool {
    line.starts_with("--- Session ") && line.contains("is suspended as its log is full")
}

fn id_after(text: &str, prefix: &str) -> Option<i64> {
    let digits = text.strip_prefix(prefix)?;
    let end = digits.find(|c: char| !c.is_ascii_digit()).unwrap_or(digits.len());
    digits[..end].parse().ok()
}

/// `      0 ms, 4 fetch(es)`: the time, and each counter that was not zero
fn performance(line: &str, event: &mut TraceEvent) -> bool {
    let mut parts = line.trim().split(", ");
    let Some(ms) = parts.next().and_then(|p| p.strip_suffix(" ms")).and_then(|n| n.parse().ok()) else {
        return false;
    };
    event.ms = Some(ms);
    for part in parts {
        let Some((count, what)) = part.split_once(' ') else { continue };
        let Ok(count) = count.parse::<u64>() else { continue };
        match what {
            "read(s)" => event.reads = count,
            "write(s)" => event.writes = count,
            "fetch(es)" => event.fetches = count,
            "mark(s)" => event.marks = count,
            _ => {}
        }
    }
    true
}

fn is_param(line: &str) -> bool {
    line.strip_prefix("param").is_some_and(|rest| {
        let digits = rest.chars().take_while(char::is_ascii_digit).count();
        digits > 0 && rest[digits..].starts_with(" = ")
    })
}

fn is_table_header(line: &str) -> bool {
    line.starts_with("Table ") && line.contains("Natural") && line.contains("Expunge")
}

/// Eight counters of ten characters each, after the table's name
const COUNTERS: usize = 8;
const COUNTER_WIDTH: usize = 10;

fn table_counts(row: &str, name_width: usize) -> TableCounts {
    let chars: Vec<char> = row.chars().collect();
    let name: String = chars.iter().take(name_width).collect();
    let mut counts = [0u64; COUNTERS];
    for (i, count) in counts.iter_mut().enumerate() {
        let from = name_width + i * COUNTER_WIDTH;
        let cell: String = chars.iter().skip(from).take(COUNTER_WIDTH).collect();
        *count = cell.trim().parse().unwrap_or(0);
    }
    let [natural, index, update, insert, delete, backout, purge, expunge] = counts;
    TableCounts { name: name.trim().to_string(), natural, index, update, insert, delete, backout, purge, expunge }
}

/// `\t/data/x.fdb (ATT_17, SYSDBA:NONE, UTF8, TCPv4:10.0.0.5/53122)`
fn attachment(line: &str, event: &mut TraceEvent) -> bool {
    let Some(open) = line.rfind(" (ATT_") else { return false };
    let Some(inner) = line[open + 2..].trim_end().strip_suffix(')') else { return false };
    event.database = line[..open].trim().to_string();
    let mut parts = inner.splitn(4, ", ");
    event.attachment = parts.next().and_then(|p| id_after(p, "ATT_"));
    if let Some(user) = parts.next() {
        let (user, role) = user.split_once(':').unwrap_or((user, ""));
        event.user = user.to_string();
        event.role = if role == "NONE" { String::new() } else { role.to_string() };
    }
    event.charset = parts.next().unwrap_or_default().to_string();
    let address = parts.next().unwrap_or_default();
    event.address = if address == "<internal>" { String::new() } else { address.to_string() };
    true
}

/// `\t\t(TRA_88, INIT_80, CONCURRENCY | WAIT | READ_WRITE)`
fn transaction(line: &str, event: &mut TraceEvent) -> bool {
    let Some(inner) = line.trim().strip_prefix('(').and_then(|l| l.strip_suffix(')')) else { return false };
    let Some(id) = id_after(inner, "TRA_") else { return false };
    event.transaction = Some(id);
    let mut options = inner.split_once(", ").map(|(_, rest)| rest).unwrap_or_default();
    if options.starts_with("INIT_") {
        options = options.split_once(", ").map(|(_, rest)| rest).unwrap_or_default();
    }
    event.transaction_options = options.to_string();
    true
}

/// A line that ends the statement's text or its plan when nothing better
/// does: the text has no end mark of its own
fn ends_text(line: &str) -> bool {
    is_param(line)
        || is_table_header(line)
        || line.ends_with(" records fetched")
        || performance(line, &mut TraceEvent::default())
}

fn trimmed_block(lines: &[&str]) -> String {
    let from = lines.iter().position(|l| !l.trim().is_empty()).unwrap_or(lines.len());
    let to = lines.iter().rposition(|l| !l.trim().is_empty()).map_or(from, |i| i + 1);
    lines[from..to].join("\n")
}

/// The event a record's lines describe. `lines[0]` is the header.
fn event(lines: &[&str]) -> TraceEvent {
    let mut event = TraceEvent::default();
    let header = lines[0];
    event.at = header[..24].to_string();
    let mut name = header.split_once(") ").map(|(_, name)| name.trim()).unwrap_or_default();
    for (prefix, outcome) in [("FAILED ", TraceOutcome::Failed), ("UNAUTHORIZED ", TraceOutcome::Unauthorized)] {
        if let Some(rest) = name.strip_prefix(prefix) {
            name = rest;
            event.outcome = outcome;
        }
    }
    for kind in ["ERROR", "WARNING"] {
        if let Some(place) = name.strip_prefix(kind).and_then(|rest| rest.strip_prefix(" AT ")) {
            event.kind = kind.to_string();
            event.object = place.to_string();
        }
    }
    if event.kind.is_empty() {
        event.kind = name.to_string();
    }
    let routine = event.kind.contains("TRIGGER") || event.kind.contains("PROCEDURE") || event.kind.contains("FUNCTION");

    let mut at = 1;
    while at < lines.len() {
        let line = lines[at];
        at += 1;
        if line.trim().is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix('\t') {
            if rest.starts_with('\t') {
                transaction(line, &mut event);
            } else if event.attachment.is_none() && attachment(line, &mut event) {
            } else if event.process.is_empty()
                && event.attachment.is_some()
                && rest.rsplit_once(':').is_some_and(|(_, pid)| !pid.is_empty() && pid.bytes().all(|b| b.is_ascii_digit()))
            {
                event.process = rest.to_string();
            } else if routine && event.object.is_empty() {
                // A trigger before Firebird 5: `NAME FOR TABLE (AFTER UPDATE)`
                event.object = rest.trim().to_string();
            }
            continue;
        }
        if let Some(id) = line.strip_prefix("Statement ").and_then(|l| l.strip_suffix(':')).and_then(|n| n.parse().ok()) {
            event.statement = Some(id);
            if lines.get(at).is_some_and(|l| l.starts_with("----------")) {
                at += 1;
            }
            let from = at;
            while at < lines.len() && !lines[at].starts_with("^^^^^^^^^^") && !ends_text(lines[at]) {
                at += 1;
            }
            event.sql = trimmed_block(&lines[from..at]);
            continue;
        }
        if line.starts_with("^^^^^^^^^^") {
            // The plan starts right after the marks, on their line or the next
            let first = line.trim_start_matches('^');
            let from = at;
            while at < lines.len() && !ends_text(lines[at]) {
                at += 1;
            }
            let mut plan = vec![first];
            plan.extend_from_slice(&lines[from..at]);
            event.plan = trimmed_block(&plan);
            continue;
        }
        if routine && event.object.is_empty() {
            let named = ["Procedure ", "Function ", "Trigger "].iter().find_map(|p| line.strip_prefix(p));
            if let Some(name) = named.and_then(|n| n.strip_suffix(':')) {
                event.object = name.to_string();
                continue;
            }
        }
        if is_param(line) {
            event.params.push(line.to_string());
        } else if let Some(count) = line.strip_suffix(" records fetched").and_then(|n| n.trim().parse().ok()) {
            event.records = Some(count);
        } else if is_table_header(line) {
            let name_width = line.chars().count().saturating_sub(COUNTERS * COUNTER_WIDTH);
            if lines.get(at).is_some_and(|l| l.starts_with("*****")) {
                at += 1;
            }
            while at < lines.len() && !lines[at].trim().is_empty() {
                event.tables.push(table_counts(lines[at], name_width));
                at += 1;
            }
        } else if !performance(line, &mut event) && matches!(event.kind.as_str(), "ERROR" | "WARNING") {
            if !event.message.is_empty() {
                event.message.push('\n');
            }
            event.message.push_str(line.trim());
        }
    }
    event
}

/// Takes a trace's text in the pieces it arrives in and gives back its
/// events
#[derive(Default)]
pub struct TraceReader {
    /// The line not ended yet
    rest: Vec<u8>,
    /// The lines of the record not ended yet
    record: Vec<String>,
}

impl TraceReader {
    pub fn new() -> Self {
        Self::default()
    }

    /// More text. `on_item` gets each thing found, with the text it was
    /// read from.
    pub fn feed(&mut self, data: &[u8], on_item: &mut dyn FnMut(TraceItem, &str)) {
        self.rest.extend_from_slice(data);
        while let Some(end) = self.rest.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.rest.drain(..=end).collect();
            let line = &line[..line.len() - 1];
            self.line(text(line.strip_suffix(b"\r").unwrap_or(line)), on_item);
        }
    }

    /// The server has nothing more for now: a record that reads as whole is
    /// handed over without waiting for the next one to begin
    pub fn pause(&mut self, on_item: &mut dyn FnMut(TraceItem, &str)) {
        if self.rest.is_empty() && self.record.last().is_some_and(|l| l.is_empty()) {
            self.flush(on_item);
        }
    }

    /// The text ended. A session stopped while the server was writing ends
    /// inside a line; when that line is a record's first, the record was
    /// cut before it said what it is, and nothing of it is read.
    pub fn finish(&mut self, on_item: &mut dyn FnMut(TraceItem, &str)) {
        if !self.rest.is_empty() {
            let line = std::mem::take(&mut self.rest);
            let line = text(line.strip_suffix(b"\r").unwrap_or(&line));
            if is_header(&line) {
                self.flush(on_item);
                return;
            }
            self.line(line, on_item);
        }
        self.flush(on_item);
    }

    fn line(&mut self, line: String, on_item: &mut dyn FnMut(TraceItem, &str)) {
        if is_header(&line) {
            self.flush(on_item);
            self.record.push(line);
        } else if is_gap(&line) {
            self.flush(on_item);
            on_item(TraceItem::Gap, &line);
        } else if !self.record.is_empty() {
            self.record.push(line);
        } else if !line.trim().is_empty() {
            on_item(TraceItem::Notice(line.trim().to_string()), &line);
        }
    }

    fn flush(&mut self, on_item: &mut dyn FnMut(TraceItem, &str)) {
        if self.record.is_empty() {
            return;
        }
        let record = std::mem::take(&mut self.record);
        let lines: Vec<&str> = record.iter().map(String::as_str).collect();
        let raw = trimmed_block(&lines);
        on_item(TraceItem::Event(Box::new(event(&lines))), &raw);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DASHES: &str = "-------------------------------------------------------------------------------";
    const MARKS: &str = "^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^";

    fn read(text: &str) -> Vec<TraceItem> {
        let mut items = Vec::new();
        let mut reader = TraceReader::new();
        reader.feed(text.as_bytes(), &mut |item, _| items.push(item));
        reader.finish(&mut |item, _| items.push(item));
        items
    }

    fn events(text: &str) -> Vec<TraceEvent> {
        read(text)
            .into_iter()
            .filter_map(|item| match item {
                TraceItem::Event(event) => Some(*event),
                _ => None,
            })
            .collect()
    }

    fn statement() -> String {
        let header = format!("{:<32}{}", "Table", "   Natural     Index    Update    Insert    Delete   Backout     Purge   Expunge");
        let stars = "*".repeat(112);
        let row = format!("{:<32}{:>10}{:>10}{:>10}", "PEDIDO_ITEM", "", 212, 3);
        [
            "2024-05-01T10:15:42.1234 (12345:0x7f8a1c0) EXECUTE_STATEMENT_FINISH",
            "\t/data/sales.fdb (ATT_17, APP:VENDAS, UTF8, TCPv4:10.0.0.5/53122)",
            "\t/usr/bin/app:4242",
            "\t\t(TRA_88, INIT_80, READ_COMMITTED | REC_VERSION | WAIT 10 | READ_WRITE)",
            "",
            "Statement 301:",
            DASHES,
            "select i.QTD",
            "",
            "  from PEDIDO_ITEM i where i.PEDIDO = ?",
            MARKS,
            "PLAN (I INDEX (FK_ITEM_PEDIDO))",
            "",
            "param0 = integer, \"1042\"",
            "param1 = varchar(10), \"<NULL>\"",
            "",
            "3 records fetched",
            "     12 ms, 2 read(s), 215 fetch(es), 4 mark(s)",
            "",
            &header,
            &stars,
            &row,
            "",
            "",
        ]
        .join("\n")
    }

    #[test]
    fn a_statement_is_read_with_its_plan_parameters_and_counters() {
        let event = events(&statement()).remove(0);
        assert_eq!(event.at, "2024-05-01T10:15:42.1234");
        assert_eq!((event.kind.as_str(), event.outcome), ("EXECUTE_STATEMENT_FINISH", TraceOutcome::Ok));
        assert_eq!((event.database.as_str(), event.attachment), ("/data/sales.fdb", Some(17)));
        assert_eq!((event.user.as_str(), event.role.as_str(), event.charset.as_str()), ("APP", "VENDAS", "UTF8"));
        assert_eq!((event.address.as_str(), event.process.as_str()), ("TCPv4:10.0.0.5/53122", "/usr/bin/app:4242"));
        assert_eq!(event.transaction, Some(88));
        assert_eq!(event.transaction_options, "READ_COMMITTED | REC_VERSION | WAIT 10 | READ_WRITE");
        assert_eq!(event.statement, Some(301));
        assert_eq!(event.sql, "select i.QTD\n\n  from PEDIDO_ITEM i where i.PEDIDO = ?");
        assert_eq!(event.plan, "PLAN (I INDEX (FK_ITEM_PEDIDO))");
        assert_eq!(event.params, ["param0 = integer, \"1042\"", "param1 = varchar(10), \"<NULL>\""]);
        assert_eq!((event.records, event.ms), (Some(3), Some(12)));
        assert_eq!((event.reads, event.writes, event.fetches, event.marks), (2, 0, 215, 4));
        let table = TableCounts { name: "PEDIDO_ITEM".into(), index: 212, update: 3, ..TableCounts::default() };
        assert_eq!(event.tables, [table]);
    }

    #[test]
    fn the_pieces_it_arrives_in_do_not_matter() {
        let text = format!("Trace session ID 7 started\n{}{}", statement(), statement()).replace('\n', "\r\n");
        let whole = read(&text);
        assert_eq!(whole.len(), 3);
        assert_eq!(whole[0], TraceItem::Notice("Trace session ID 7 started".into()));
        let mut items = Vec::new();
        let mut reader = TraceReader::new();
        for piece in text.as_bytes().chunks(7) {
            reader.feed(piece, &mut |item, _| items.push(item));
        }
        reader.finish(&mut |item, _| items.push(item));
        assert_eq!(items, whole);
    }

    #[test]
    fn a_record_is_handed_over_when_the_server_pauses_after_it() {
        let text = statement();
        let mut items = Vec::new();
        let mut raw = String::new();
        let mut reader = TraceReader::new();
        let (start, end) = text.split_at(text.len() - 40);
        reader.feed(start.as_bytes(), &mut |item, _| items.push(item));
        reader.pause(&mut |item, _| items.push(item));
        assert!(items.is_empty());
        reader.feed(end.as_bytes(), &mut |item, _| items.push(item));
        reader.pause(&mut |item, text| {
            items.push(item);
            raw = text.to_string();
        });
        assert_eq!(items.len(), 1);
        assert!(raw.starts_with("2024-05-01T10:15:42.1234 (") && raw.ends_with("3"), "{raw}");
    }

    #[test]
    fn a_record_cut_in_its_first_line_is_not_read() {
        // A session stopped while the server was writing: the text ends
        // inside the next record's first line, before its kind
        let text = "2024-05-01T10:15:40.0001 (12345:0x7f8a1c0) START_TRANSACTION\n\
                    \t/data/sales.fdb (ATT_17, SYSDBA:NONE, NONE, <internal>)\n\
                    \t\t(TRA_88, CONCURRENCY | NOWAIT | READ_ONLY)\n\n\
                    2024-05-01T10:15:40.0002 (12345:0x7f8a";
        let events = events(text);
        assert_eq!(events.len(), 1, "{events:?}");
        assert_eq!(events[0].kind, "START_TRANSACTION");
        // Cut after its first line, a record is still read for what it says
        let events = super::tests::events("2024-05-01T10:15:40.0001 (12345:0x7f8a1c0) START_TRANSACTION\n\t/data/sal");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, "START_TRANSACTION");
    }

    #[test]
    fn transactions_and_connections_have_no_statement() {
        let text = "2024-05-01T10:15:40.0001 (12345:0x7f8a1c0) START_TRANSACTION\n\
                    \t/data/sales.fdb (ATT_17, SYSDBA:NONE, NONE, <internal>)\n\
                    \t\t(TRA_88, CONCURRENCY | NOWAIT | READ_ONLY)\n\n\
                    2024-05-01T10:15:40.0002 (12345:0x7f8a1c0) FAILED ATTACH_DATABASE\n\
                    \tC:\\data\\sales.fdb (ATT_0, <unknown_user>, NONE, TCPv6:::1/5000)\n\n";
        let events = events(text);
        assert_eq!(events[0].kind, "START_TRANSACTION");
        assert_eq!((events[0].role.as_str(), events[0].address.as_str()), ("", ""));
        assert_eq!(events[0].transaction_options, "CONCURRENCY | NOWAIT | READ_ONLY");
        assert_eq!((events[1].kind.as_str(), events[1].outcome), ("ATTACH_DATABASE", TraceOutcome::Failed));
        assert_eq!(events[1].database, "C:\\data\\sales.fdb");
        assert_eq!((events[1].user.as_str(), events[1].address.as_str()), ("<unknown_user>", "TCPv6:::1/5000"));
        assert!(events[1].process.is_empty() && events[1].sql.is_empty());
    }

    #[test]
    fn routines_are_named_in_each_version_s_way() {
        let attachment = "\t/data/sales.fdb (ATT_17, SYSDBA:NONE, UTF8, TCPv4:10.0.0.5/53122)\n\t\t(TRA_88, CONCURRENCY | WAIT | READ_WRITE)\n";
        let text = format!(
            "2024-05-01T10:15:40.0001 (1:0x1) EXECUTE_PROCEDURE_FINISH\n{attachment}\nProcedure P_TOTAL:\nparam0 = integer, \"7\"\n\n      3 ms, 9 fetch(es)\n\n\
             2024-05-01T10:15:40.0002 (1:0x1) EXECUTE_TRIGGER_FINISH\n{attachment}\tPEDIDO_AU FOR PEDIDO (AFTER UPDATE) \n      0 ms\n\n\
             2024-05-01T10:15:40.0003 (1:0x1) EXECUTE_TRIGGER_FINISH\n{attachment}\nTrigger PEDIDO_AU FOR PEDIDO (AFTER UPDATE):\n      1 ms\n\n\
             2024-05-01T10:15:40.0004 (1:0x1) EXECUTE_FUNCTION_START\n{attachment}\nFunction F_NOME:\n\n"
        );
        let events = events(&text);
        assert_eq!((events[0].object.as_str(), events[0].ms, events[0].fetches), ("P_TOTAL", Some(3), 9));
        assert_eq!(events[0].params.len(), 1);
        assert_eq!((events[1].object.as_str(), events[1].ms), ("PEDIDO_AU FOR PEDIDO (AFTER UPDATE)", Some(0)));
        assert_eq!((events[2].object.as_str(), events[2].ms), ("PEDIDO_AU FOR PEDIDO (AFTER UPDATE)", Some(1)));
        assert_eq!((events[3].kind.as_str(), events[3].object.as_str()), ("EXECUTE_FUNCTION_START", "F_NOME"));
    }

    #[test]
    fn errors_gaps_and_what_is_not_known_are_all_kept() {
        let text = "2024-05-01T10:15:40.0001 (1:0000000001234567) ERROR AT JStatement::execute\n\
                    \t/data/sales.fdb (ATT_17, SYSDBA:NONE, UTF8, TCPv4:10.0.0.5/53122)\n\
                    335544345 : lock conflict on no wait transaction\n\n\
                    --- Session 7 is suspended as its log is full ---\n\
                    2024-05-01T10:15:41.0001 (1:0000000001234567) SOMETHING_NEW\n\tnot an attachment\n\n";
        let mut raw = Vec::new();
        let mut items = Vec::new();
        let mut reader = TraceReader::new();
        reader.feed(text.as_bytes(), &mut |item, text| {
            items.push(item);
            raw.push(text.to_string());
        });
        reader.finish(&mut |item, text| {
            items.push(item);
            raw.push(text.to_string());
        });
        let TraceItem::Event(error) = &items[0] else { panic!("{items:?}") };
        assert_eq!((error.kind.as_str(), error.object.as_str()), ("ERROR", "JStatement::execute"));
        assert_eq!(error.message, "335544345 : lock conflict on no wait transaction");
        assert!(raw[0].ends_with("lock conflict on no wait transaction"), "{}", raw[0]);
        assert_eq!(items[1], TraceItem::Gap);
        let TraceItem::Event(unknown) = &items[2] else { panic!("{items:?}") };
        assert_eq!((unknown.kind.as_str(), unknown.attachment), ("SOMETHING_NEW", None));
        assert!(raw[2].contains("not an attachment"));
    }

    #[test]
    fn an_explained_plan_keeps_its_lines() {
        let text = format!(
            "2024-05-01T10:15:40.0001 (1:0x1) EXECUTE_STATEMENT_FINISH\n\
             \t/data/sales.fdb (ATT_17, SYSDBA:NONE, UTF8, TCPv4:10.0.0.5/53122)\n\n\
             Statement 5:\n{DASHES}\nselect count(*) from PEDIDO_ITEM\n{MARKS}\n\
             Select Expression\n    -> Aggregate\n        -> Table \"PEDIDO_ITEM\" Full Scan\n\n\
             1 records fetched\n     33 ms, 48210 fetch(es)\n\n"
        );
        let event = events(&text).remove(0);
        assert_eq!(event.sql, "select count(*) from PEDIDO_ITEM");
        assert_eq!(event.plan, "Select Expression\n    -> Aggregate\n        -> Table \"PEDIDO_ITEM\" Full Scan");
        assert_eq!((event.records, event.ms, event.fetches), (Some(1), Some(33), 48210));
    }
}
