// What a statement's counters add up to: the figures the Performance
// Analysis tab leads with, and what stands out in them, said in words.
//
// No imports but types: `npm test` runs this file as it is.
import type { Performance, TablePerformance } from "./api";

export interface PerfInput {
  prepareMs: number;
  executeMs: number;
  /** `null` when the statement returned no rows to fetch */
  fetchMs: number | null;
  /** Rows fetched, of a statement that returns them */
  rows: number | null;
  performance: Performance | null;
  /** The relations the statement names as its sources */
  named: string[];
}

export interface TableRead extends TablePerformance {
  read: number;
  changed: number;
  /** Not named in the statement: a trigger, a procedure or a view got there */
  indirect: boolean;
}

export interface Finding {
  kind: string;
  tone: "danger" | "warn" | "info" | "ok" | "muted";
  /** With `{holes}` for `vars`, as the UI's texts are written */
  text: string;
  vars: Record<string, string | number>;
}

export interface PerfSummary {
  totalMs: number;
  tables: TableRead[];
  read: number;
  /** Rows inserted, updated and deleted, in every table */
  changed: { inserted: number; updated: number; deleted: number; total: number; tables: number };
  cleaned: { purges: number; expunges: number; backouts: number };
  /** What the reads are counted against: rows returned, else rows changed */
  per: { rows: number; what: "returned" | "changed" } | null;
  /** Of the pages touched, the share found in the cache, 0 to 1 */
  cached: number | null;
  findings: Finding[];
}

const n = (value: number) => Math.round(value).toLocaleString("en-US");

/** A table read whole is worth saying from this many records on, when
 *  they are this many times the rows that came of it */
const SCAN_RECORDS = 1000;
const SCAN_TIMES = 10;
/** Records read through an index for each row, before a join is pointed at */
const JOIN_TIMES = 50;
const PREPARE_MS = 20;
const DISK_PAGES = 100;

export function summarize(input: PerfInput): PerfSummary {
  const perf = input.performance;
  const totalMs = input.prepareMs + input.executeMs + (input.fetchMs ?? 0);
  const named = new Set(input.named);
  const tables: TableRead[] = (perf?.tables ?? [])
    .map((table) => ({
      ...table,
      read: table.indexedReads + table.nonIndexedReads,
      changed: table.inserts + table.updates + table.deletes,
      indirect: named.size > 0 && !named.has(table.table),
    }))
    .sort((a, b) => b.read - a.read || b.changed - a.changed);
  const sum = (of: (table: TableRead) => number) => tables.reduce((total, table) => total + of(table), 0);
  const changed = {
    inserted: sum((x) => x.inserts),
    updated: sum((x) => x.updates),
    deleted: sum((x) => x.deletes),
    total: sum((x) => x.changed),
    tables: tables.filter((x) => x.changed > 0).length,
  };
  const cleaned = { purges: sum((x) => x.purges), expunges: sum((x) => x.expunges), backouts: sum((x) => x.backouts) };
  const read = sum((x) => x.read);
  const per: PerfSummary["per"] =
    changed.total > 0 ? { rows: changed.total, what: "changed" } : input.rows ? { rows: input.rows, what: "returned" } : null;
  const cached = perf && perf.fetches > 0 ? Math.max(0, 1 - perf.reads / perf.fetches) : null;

  const findings: Finding[] = [];
  const rows = per?.rows ?? 1;
  const whole = tables.filter((x) => x.nonIndexedReads >= SCAN_RECORDS && x.nonIndexedReads >= rows * SCAN_TIMES);
  for (const table of whole.slice(0, 2)) {
    findings.push({
      kind: "Scan",
      tone: "danger",
      text:
        per?.what === "changed"
          ? "{table} was read whole: {records} records without an index, to change {rows}. An index on what the WHERE compares would read only those."
          : "{table} was read whole: {records} records without an index, for {rows} row(s) returned. An index on what the WHERE or the join compares would read only those.",
      vars: { table: table.table, records: n(table.nonIndexedReads), rows: n(per?.rows ?? 0) },
    });
  }
  if (input.prepareMs >= PREPARE_MS && input.prepareMs >= totalMs / 2) {
    findings.push({
      kind: "Prepare",
      tone: "warn",
      text: "Preparing took {prepare} of the {total} ms. That is the cost of a first run: the server read the tables' definitions and chose a plan. Run it again to see it warm.",
      vars: { prepare: n(input.prepareMs), total: n(totalMs) },
    });
  }
  if (perf && perf.reads >= DISK_PAGES) {
    findings.push({
      kind: "Disk",
      tone: "warn",
      text: "{pages} pages came from disk. A second run finds them in the cache and says what the statement costs warm.",
      vars: { pages: n(perf.reads) },
    });
  }
  const join = per?.what === "returned" ? tables.find((x) => !whole.includes(x) && x.indexedReads >= per.rows * JOIN_TIMES) : undefined;
  if (join && per) {
    findings.push({
      kind: "Join",
      tone: "info",
      text: "{table}: {times} records read for each row returned, through an index.",
      vars: { table: join.table, times: n(join.indexedReads / per.rows) },
    });
  }
  const indirect = tables.filter((x) => x.indirect);
  if (indirect.length > 0) {
    findings.push({
      kind: "Indirect",
      tone: "info",
      text:
        indirect.length === 1
          ? "{tables} is not named in the statement: a trigger, a procedure or a view got there."
          : "{tables} are not named in the statement: triggers, procedures or views got there.",
      vars: { tables: indirect.slice(0, 4).map((x) => x.table).join(", ") + (indirect.length > 4 ? "…" : "") },
    });
  }
  if (cleaned.purges + cleaned.expunges + cleaned.backouts > 0) {
    findings.push({
      kind: "Cleaning",
      tone: "muted",
      text: "{versions} old record version(s) were cleaned on the way: this statement paid for garbage others left.",
      vars: { versions: n(cleaned.purges + cleaned.expunges + cleaned.backouts) },
    });
  }
  if (perf && !findings.some((f) => f.tone === "danger" || f.tone === "warn")) {
    const scanned = tables.some((x) => x.nonIndexedReads > 0);
    findings.push({
      kind: "Fine",
      tone: "ok",
      text: scanned
        ? "Nothing read from disk, and what was read without an index was little."
        : perf.reads > 0
          ? "No table read without an index, and little read from disk."
          : "No table read without an index, and nothing read from disk.",
      vars: {},
    });
  }
  return { totalMs, tables, read, changed, cleaned, per, cached, findings };
}
