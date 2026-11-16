// The activity monitor: a recording of everything the server runs, and
// what it adds up to. The backend records and keeps; this asks for what it
// shows. The grids of the monitoring tables (who is connected now, with
// the actions on them) are one of its tabs.
import { useVirtualizer } from "@tanstack/react-virtual";
import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import {
  api,
  errorText,
  type TraceDetail,
  type TraceFilter,
  type TraceMemory,
  type TraceRecorded,
  type TraceRow,
  type TraceSecond,
  type TraceStatement,
} from "../api";
import type { AdminPanel } from "../admin";
import { openEditor } from "../dock";
import { t } from "../i18n";
import { Icon } from "../icons";
import { openRecorded, startMonitor, stopMonitor, useMonitor, type MonitorSession } from "../monitor";
import { useStore } from "../store";
import { TextButton } from "./ObjectViewer";

type Tab = "overview" | "live" | "statements" | "connections" | "memory" | "sessions";
const tabs: [Tab, string][] = [
  ["overview", "Overview"],
  ["live", "Live"],
  ["statements", "Statements"],
  ["connections", "Connections and transactions"],
  ["memory", "Sorts and memory"],
  ["sessions", "Recorded sessions"],
];

const standings = {
  recording: "Recording",
  stopped: "Stopped",
  ended: "Ended by the server",
  failed: "Failed",
  opened: "Recorded session",
} as const;

const n = (value: number) => Math.round(value).toLocaleString();

/** A count in a few characters: 86 k, 1.2 M */
function short(value: number): string {
  if (value >= 1e9) return `${(value / 1e9).toFixed(1)} G`;
  if (value >= 1e6) return `${(value / 1e6).toFixed(1)} M`;
  if (value >= 1e4) return `${Math.round(value / 1e3)} k`;
  return n(value);
}

function took(ms: number): string {
  if (ms >= 60_000) return `${(ms / 60_000).toFixed(1)} min`;
  if (ms >= 1000) return `${(ms / 1000).toFixed(ms >= 10_000 ? 1 : 2)} s`;
  return `${ms >= 10 || Number.isInteger(ms) ? Math.round(ms) : ms.toFixed(1)} ms`;
}

function bytes(value: number): string {
  if (value >= 1 << 30) return `${(value / (1 << 30)).toFixed(2)} GB`;
  if (value >= 1 << 20) return `${Math.round(value / (1 << 20))} MB`;
  return `${Math.round(value / 1024)} KB`;
}

function clock(seconds: number): string {
  const h = Math.floor(seconds / 3600);
  const m = Math.floor((seconds % 3600) / 60);
  const two = (v: number) => String(v).padStart(2, "0");
  return `${two(h)}:${two(m)}:${two(Math.floor(seconds % 60))}`;
}

/** The time of day of a second on the server's clock */
const timeOf = (at: number) => clock(((at % 86400) + 86400) % 86400);

const selectClass = "h-6 rounded border border-line bg-panel px-1 text-fg disabled:text-faint";
const inputClass = "h-6 rounded border border-line bg-panel px-1.5 text-fg placeholder:text-faint";
const mono = "font-mono text-[11.5px]";
const card = "rounded-md border border-line bg-panel";

function Empty({ children }: { children: ReactNode }) {
  return <p className="px-3 py-2 text-muted">{children}</p>;
}

// ── Charts ──────────────────────────────────────────────────────────────

/** How much of a session the charts show, in seconds */
const WINDOW = 600;

/** The last seconds of a session, one entry a second: a second with no
 *  events has no entry of its own, and is one where nothing ran */
function dense(seconds: TraceSecond[]): TraceSecond[] {
  if (seconds.length === 0) return [];
  const to = seconds[seconds.length - 1].at;
  const from = Math.max(seconds[0].at, to - WINDOW + 1);
  const by = new Map(seconds.map((s) => [s.at, s]));
  const out: TraceSecond[] = [];
  for (let at = from; at <= to; at++) {
    out.push(
      by.get(at) ?? { at, events: 0, statements: 0, errors: 0, connects: 0, average: 0, high: 0, fetches: 0, reads: 0, writes: 0, marks: 0, attachments: 0 },
    );
  }
  return out;
}

/** A round number at or above `value`, for the top of an axis */
function ceiling(value: number): number {
  if (value <= 0) return 1;
  const unit = 10 ** Math.floor(Math.log10(value));
  return [1, 2, 5, 10].map((m) => m * unit).find((top) => top >= value) ?? value;
}

interface Series {
  label: string;
  values: number[];
}

function Chart({ title, seconds, series }: { title: string; seconds: TraceSecond[]; series: Series[] }) {
  const top = ceiling(Math.max(0, ...series.flatMap((s) => s.values)));
  const last = Math.max(1, seconds.length - 1);
  const points = (values: number[]) =>
    values.map((v, i) => `${((i * 600) / last).toFixed(1)},${(132 - (Math.min(v, top) / top) * 124).toFixed(1)}`).join(" ");
  const ends = seconds.length > 0 ? [seconds[0].at, seconds[Math.floor(last / 2)].at, seconds[seconds.length - 1].at] : [];
  return (
    <div className={`${card} flex min-w-0 flex-col gap-1.5 px-3 py-2.5`}>
      <div className="flex flex-wrap items-baseline gap-3">
        <span className="font-semibold">{title}</span>
        <span className="flex-1" />
        {series.map((s, i) => (
          <span key={s.label} className="flex items-center gap-1.5 text-muted">
            <span
              aria-hidden
              className="w-3.5 border-t-2"
              style={{ borderColor: `var(--chart-${i + 1})`, borderStyle: i ? "dashed" : "solid" }}
            />
            {s.label}
          </span>
        ))}
      </div>
      <div className="flex gap-2">
        <div className="flex h-[140px] w-9 flex-col justify-between text-right text-[11px] text-muted tabular-nums">
          <span>{short(top)}</span>
          <span>{short(top / 2)}</span>
          <span>0</span>
        </div>
        <svg role="img" aria-label={title} viewBox="0 0 600 140" preserveAspectRatio="none" className="block h-[140px] min-w-0 flex-1">
          {[8, 70, 132].map((y) => (
            <line key={y} x1="0" x2="600" y1={y} y2={y} stroke="var(--border)" strokeWidth="1" vectorEffect="non-scaling-stroke" />
          ))}
          {series
            .map((s, i) => (
              <polyline
                key={s.label}
                points={points(s.values)}
                fill="none"
                stroke={`var(--chart-${i + 1})`}
                strokeWidth="1.6"
                strokeDasharray={i ? "5 3" : undefined}
                strokeLinejoin="round"
                vectorEffect="non-scaling-stroke"
              />
            ))
            .reverse()}
        </svg>
      </div>
      <div className="flex justify-between pl-11 text-[11px] text-muted tabular-nums">
        {ends.map((at, i) => (
          <span key={i}>{timeOf(at)}</span>
        ))}
      </div>
    </div>
  );
}

// ── Statements ──────────────────────────────────────────────────────────

const orders: [string, string][] = [
  ["time", "Total time"],
  ["runs", "Runs"],
  ["fetches", "Page fetches"],
  ["natural", "Natural reads"],
  ["slowest", "Slowest run"],
  ["memory", "Peak memory"],
];

/** The statements that cost the most, asked again as the session goes on */
function useTop(session: MonitorSession, by: string, sortingOnly: boolean, limit: number): TraceStatement[] {
  const [top, setTop] = useState<TraceStatement[]>([]);
  // Asked again every other second of a recording, not at every event
  const [beat, setBeat] = useState(0);
  const recording = session.tick.standing === "recording";
  useEffect(() => {
    if (!recording) return;
    const timer = setInterval(() => setBeat((v) => v + 1), 2000);
    return () => clearInterval(timer);
  }, [recording]);
  useEffect(() => {
    let live = true;
    void api.traceTop(session.run, by, sortingOnly, limit).then(
      (rows) => live && setTop(rows),
      () => {},
    );
    return () => {
      live = false;
    };
  }, [session.run, by, sortingOnly, limit, beat, session.tick.standing]);
  return top;
}

function StatementTable(props: { rows: TraceStatement[]; by: string; memory?: boolean; none?: string; open(seq: number): void }) {
  const { rows, by } = props;
  const key = (s: TraceStatement) =>
    by === "runs" ? s.runs : by === "fetches" ? s.fetches : by === "natural" ? s.natural : by === "slowest" ? s.slowest : by === "memory" ? s.memory : s.ms;
  const most = Math.max(1, ...rows.map(key));
  if (rows.length === 0) return <Empty>{props.none ?? t("No statement finished yet.")}</Empty>;
  return (
    <div className="scroll-thin overflow-x-auto">
      <table className="w-full min-w-[720px] border-collapse tabular-nums">
        <thead>
          <tr className="text-right whitespace-nowrap text-muted">
            <th className="px-3 py-1.5 text-left font-medium">{t("Statement, literals replaced by ?")}</th>
            <th className="px-2 py-1.5 font-medium">{t("Runs")}</th>
            <th className="px-2 py-1.5 font-medium">{t("Total")}</th>
            <th className="px-2 py-1.5 font-medium">{t("Average")}</th>
            <th className="px-2 py-1.5 font-medium">{t("Slowest")}</th>
            <th className="px-2 py-1.5 font-medium">{t("Fetches")}</th>
            <th className="px-2 py-1.5 font-medium">{t("Natural reads")}</th>
            {props.memory && <th className="px-3 py-1.5 font-medium">{t("Peak memory")}</th>}
          </tr>
        </thead>
        <tbody>
          {rows.map((s) => (
            <tr
              key={s.sql}
              className="cursor-pointer border-t border-line text-right whitespace-nowrap hover:bg-hover"
              title={t("Show its slowest run")}
              onClick={() => props.open(s.slowestEvent)}
            >
              <td className="w-full max-w-0 min-w-[240px] px-3 py-1.5 text-left">
                <div className={`${mono} truncate`}>{s.sql}</div>
                <div aria-hidden className="mt-1 h-[3px] rounded-sm bg-[var(--chart-1)]" style={{ width: `${Math.max(1, (key(s) / most) * 100)}%` }} />
              </td>
              <td className="px-2 py-1.5">{n(s.runs)}</td>
              <td className="px-2 py-1.5 font-semibold">{took(s.ms)}</td>
              <td className="px-2 py-1.5">{took(s.ms / Math.max(1, s.runs))}</td>
              <td className="px-2 py-1.5">{took(s.slowest)}</td>
              <td className="px-2 py-1.5">{short(s.fetches)}</td>
              <td className={`px-2 py-1.5 ${s.natural > 0 ? "text-danger" : ""}`}>{s.natural > 0 ? short(s.natural) : ""}</td>
              {props.memory && <td className="px-3 py-1.5 font-semibold">{s.memory > 0 ? bytes(s.memory) : ""}</td>}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

function OrderSelect({ by, setBy, memory }: { by: string; setBy(by: string): void; memory?: boolean }) {
  return (
    <label className="flex items-center gap-1.5 text-muted">
      {t("By")}
      <select className={selectClass} value={by} onChange={(e) => setBy(e.target.value)}>
        {orders
          .filter(([id]) => memory || id !== "memory")
          .map(([id, label]) => (
            <option key={id} value={id}>
              {t(label)}
            </option>
          ))}
      </select>
    </label>
  );
}

// ── Overview ────────────────────────────────────────────────────────────

interface Finding {
  kind: string;
  tone: string;
  text: string;
  where: string;
  seq?: number;
}

/** What stands out in the statements that cost the most */
function findings(top: TraceStatement[], sorting: TraceStatement[], errors: number, session: MonitorSession): Finding[] {
  const found: Finding[] = [];
  // A program that connects for every statement or two: connecting costs
  // more than most statements do
  const connects = session.seconds.reduce((sum, s) => sum + s.connects, 0);
  const statements = session.seconds.reduce((sum, s) => sum + s.statements, 0);
  if (connects >= 20 && statements < connects * 5) {
    const [who, count] = session.tick.connector ?? ["", 0];
    found.push({
      kind: t("Connect"),
      tone: "text-warn",
      text: t("{connects} connections were made for {statements} statements. Making a connection costs more than most statements: keeping it open, or a pool of them, saves that.", {
        connects: n(connects),
        statements: n(statements),
      }),
      where: who ? t("{count} of them by {who}", { count: n(count), who }) : "",
    });
  }
  for (const s of top) {
    const each = s.natural / Math.max(1, s.runs);
    if (each >= 1000 && s.natural > s.records * 10) {
      found.push({
        kind: t("Scan"),
        tone: "text-danger",
        text: t("{each} records read without an index at every run, {records} returned.", {
          each: n(each),
          records: n(s.records / Math.max(1, s.runs)),
        }),
        where: `${s.sql} · ${t("{n} runs", { n: n(s.runs) })}`,
        seq: s.slowestEvent,
      });
    }
  }
  for (const s of sorting.filter((s) => s.memory >= 16 << 20 || s.slowest >= 1000).slice(0, 3)) {
    found.push({
      kind: t("Sort"),
      tone: "text-warn",
      text: s.memory > 0 ? t("A sort that peaked at {memory} of memory.", { memory: bytes(s.memory) }) : t("A sort whose slowest run took {time}.", { time: took(s.slowest) }),
      where: s.sql,
      seq: s.slowestEvent,
    });
  }
  if (errors > 0) found.push({ kind: t("Error"), tone: "text-danger", text: t("{n} events failed.", { n: n(errors) }), where: t("See them in Live, filtered by errors.") });
  for (const s of top.filter((s) => s.runs >= 1000).slice(0, 2)) {
    found.push({
      kind: t("Hot"),
      tone: "text-muted",
      text: t("Ran {runs} times at {each} each: {total} in all.", { runs: n(s.runs), each: took(s.ms / s.runs), total: took(s.ms) }),
      where: s.sql,
      seq: s.slowestEvent,
    });
  }
  return found.slice(0, 8);
}

function Overview({ session, open }: { session: MonitorSession; open(seq: number): void }) {
  const [by, setBy] = useState("time");
  const top = useTop(session, by, false, 8);
  const byNatural = useTop(session, "natural", false, 5);
  const sorting = useTop(session, "memory", true, 5);
  const seconds = useMemo(() => dense(session.seconds), [session.seconds]);
  // The last ten seconds, the one still filling left out
  const recent = seconds.slice(-11, -1);
  const mean = (pick: (s: TraceSecond) => number) => (recent.length ? recent.reduce((sum, s) => sum + pick(s), 0) / recent.length : 0);
  const statements = recent.reduce((sum, s) => sum + s.statements, 0);
  const average = statements ? recent.reduce((sum, s) => sum + s.average * s.statements, 0) / statements : 0;
  const errors = session.seconds.reduce((sum, s) => sum + s.errors, 0);
  const found = findings(byNatural, sorting, errors, session);
  const tiles: [string, string, string, string?][] = [
    [t("Statements per second"), n(mean((s) => s.statements)), t("{n} events recorded", { n: n(session.tick.total) })],
    [t("Average statement time"), took(average), t("95 in 100 under {time}", { time: took(Math.max(0, ...recent.map((s) => s.high))) })],
    [t("Page fetches per second"), short(mean((s) => s.fetches)), t("{n} read from disk", { n: short(mean((s) => s.reads)) })],
    [t("Active connections"), n(Math.max(0, ...recent.map((s) => s.attachments))), t("that ran something lately")],
    [
      t("Connections made per second"),
      mean((s) => s.connects).toFixed(1),
      t("{n} in this session", { n: n(session.seconds.reduce((sum, s) => sum + s.connects, 0)) }),
    ],
    [t("Errors"), n(errors), errors ? t("in this session") : t("none in this session"), errors ? "text-danger" : undefined],
  ];
  return (
    <div className="scroll-thin flex h-full flex-col gap-3 overflow-auto bg-bg p-3">
      <div className="grid grid-cols-[repeat(auto-fit,minmax(170px,1fr))] gap-3">
        {tiles.map(([label, value, note, tone]) => (
          <div key={label} className={`${card} flex flex-col gap-0.5 px-3 py-2.5`}>
            <span className="text-muted">{label}</span>
            <span className="text-[22px] font-semibold tabular-nums">{value}</span>
            <span className={tone ?? "text-muted"}>{note}</span>
          </div>
        ))}
      </div>
      <div className="grid grid-cols-[repeat(auto-fit,minmax(max(420px,40%),1fr))] gap-3">
        <Chart title={t("Statements per second")} seconds={seconds} series={[{ label: t("finished"), values: seconds.map((s) => s.statements) }]} />
        <Chart
          title={t("Statement time, ms")}
          seconds={seconds}
          series={[
            { label: t("average"), values: seconds.map((s) => s.average) },
            { label: t("95 in 100 under"), values: seconds.map((s) => s.high) },
          ]}
        />
        <Chart
          title={t("Pages per second")}
          seconds={seconds}
          series={[
            { label: t("fetches from cache"), values: seconds.map((s) => s.fetches) },
            { label: t("reads from disk"), values: seconds.map((s) => s.reads) },
          ]}
        />
        <Chart
          title={t("Connections")}
          seconds={seconds}
          series={[
            { label: t("that ran something"), values: seconds.map((s) => s.attachments) },
            { label: t("made"), values: seconds.map((s) => s.connects) },
          ]}
        />
      </div>
      <div className="flex flex-wrap items-start gap-3">
        <div className={`${card} min-w-0 flex-[999_1_620px]`}>
          <div className="flex items-center gap-2.5 border-b border-line px-3 py-2">
            <span className="font-semibold">{t("Top statements")}</span>
            <span className="flex-1" />
            <OrderSelect by={by} setBy={setBy} />
          </div>
          <StatementTable rows={top} by={by} open={open} />
        </div>
        <div className={`${card} min-w-0 flex-[1_1_360px]`}>
          <div className="border-b border-line px-3 py-2 font-semibold">{t("Findings")}</div>
          {found.map((f, i) => (
            <button
              key={i}
              type="button"
              disabled={f.seq === undefined}
              onClick={() => f.seq !== undefined && open(f.seq)}
              className="flex w-full gap-2.5 border-t border-line px-3 py-2 text-left first:border-t-0 enabled:hover:bg-hover"
            >
              <span className={`w-14 shrink-0 font-semibold ${f.tone}`}>{f.kind}</span>
              <span className="flex min-w-0 flex-col gap-0.5">
                <span>{f.text}</span>
                <span className="truncate text-muted">{f.where}</span>
              </span>
            </button>
          ))}
          {found.length === 0 && <Empty>{t("Nothing stands out so far.")}</Empty>}
        </div>
      </div>
    </div>
  );
}

// ── Live ────────────────────────────────────────────────────────────────

const PAGE = 200;
const ROW_H = 22;

const groups: [string, string][] = [
  ["", "All"],
  ["statements", "Statements"],
  ["routines", "Procedures and triggers"],
  ["transactions", "Transactions"],
  ["connections", "Connections"],
  ["errors", "Errors"],
];

const kinds: Record<string, string> = {
  EXECUTE_STATEMENT_FINISH: "Statement",
  EXECUTE_STATEMENT_START: "Statement start",
  PREPARE_STATEMENT: "Prepare",
  FREE_STATEMENT: "Free",
  CLOSE_CURSOR: "Close cursor",
  START_TRANSACTION: "Transaction start",
  COMMIT_TRANSACTION: "Commit",
  ROLLBACK_TRANSACTION: "Rollback",
  COMMIT_RETAINING: "Commit retaining",
  ROLLBACK_RETAINING: "Rollback retaining",
  ATTACH_DATABASE: "Connect",
  DETACH_DATABASE: "Disconnect",
  ERROR: "Error",
  WARNING: "Warning",
};

/** An event's name as a person says it */
function kindName(kind: string): string {
  if (kinds[kind]) return t(kinds[kind]);
  const words = kind.replace(/^EXECUTE_/, "").toLowerCase().split("_");
  return t(words.join(" ").replace(/^./, (c) => c.toUpperCase()));
}

function Detail({ session, seq, registration }: { session: MonitorSession; seq: number | null; registration: string }) {
  const [detail, setDetail] = useState<TraceDetail | null>(null);
  const [gone, setGone] = useState("");
  useEffect(() => {
    setDetail(null);
    setGone("");
    if (seq === null) return;
    let live = true;
    void api.traceEvent(session.run, seq).then(
      (d) => live && setDetail(d),
      () => live && setGone(t("This event is no longer kept in memory: it is in the log.")),
    );
    return () => {
      live = false;
    };
  }, [session.run, seq]);

  if (seq === null) return <Empty>{t("Select an event to see it in full.")}</Empty>;
  if (gone) return <Empty>{gone}</Empty>;
  if (!detail) return <Empty>{t("Loading…")}</Empty>;
  const { event, statement } = detail;
  const scans = event.tables.some((table) => table.natural > 0);
  // Of what is counted per table, only what some table has: a pane this
  // narrow has no room for six columns of nothing, and a table wider than
  // it gets a scrollbar that hides its last row
  const counted = (
    [
      ["Index", "index"],
      ["Update", "update"],
      ["Insert", "insert"],
      ["Delete", "delete"],
      ["Backout", "backout"],
      ["Purge", "purge"],
      ["Expunge", "expunge"],
    ] as const
  ).filter(([, key], i) => i === 0 || event.tables.some((table) => table[key] > 0));
  const block = (label: string, text: string) =>
    text && (
      <div className="flex flex-col gap-1">
        <span className="text-muted">{label}</span>
        <pre className={`${card} ${mono} scroll-thin m-0 max-h-64 overflow-x-hidden overflow-y-auto px-2.5 py-2 leading-[1.5] [overflow-wrap:anywhere] whitespace-pre-wrap select-text`}>{text}</pre>
      </div>
    );
  const tile = (label: string, value: string, tone = "") => (
    <div className={`${card} px-2.5 py-1.5`}>
      <div className="text-muted">{label}</div>
      <div className={`text-[16px] font-semibold tabular-nums ${tone}`}>{value}</div>
    </div>
  );
  const who = [
    event.attachment !== null && `ATT_${event.attachment}`,
    event.user && (event.role ? `${event.user} (${event.role})` : event.user),
    event.address,
    event.process,
    event.transaction !== null && `TRA_${event.transaction}`,
    event.transactionOptions,
  ].filter(Boolean);
  return (
    <div className="flex flex-col gap-3 p-3">
      <div className="flex flex-wrap items-baseline gap-2">
        <span className={`font-semibold ${event.outcome !== "ok" || event.kind === "ERROR" ? "text-danger" : ""}`}>
          {kindName(event.kind)}
          {event.outcome !== "ok" && ` · ${t(event.outcome === "failed" ? "failed" : "not allowed")}`}
        </span>
        <span className="text-muted tabular-nums">{event.at.slice(11)}</span>
        <span className="flex-1" />
        {event.sql && (
          <>
            <TextButton icon="copy" label={t("Copy SQL")} onClick={() => navigator.clipboard.writeText(event.sql)} />
            <TextButton icon="sql" label={t("Open in SQL Editor")} onClick={() => openEditor(registration, event.sql)} />
          </>
        )}
      </div>
      <span className="text-muted select-text">{who.join(" · ")}</span>
      {event.object && <span className={`${mono} select-text`}>{event.object}</span>}
      {block(t("Message"), event.message)}
      {block(t("SQL"), event.sql)}
      {block(t("Parameters"), event.params.join("\n"))}
      {block(t("Plan"), event.plan)}
      {scans && <span className="text-danger">{t("Records were read without an index.")}</span>}
      {event.ms !== null && (
        <div className="grid grid-cols-[repeat(auto-fit,minmax(84px,1fr))] gap-2">
          {tile(t("Took"), took(event.ms), event.ms >= 100 ? "text-danger" : "")}
          {event.records !== null && tile(t("Records"), n(event.records))}
          {tile(t("Fetches"), n(event.fetches))}
          {tile(t("Reads"), n(event.reads))}
          {tile(t("Writes"), n(event.writes))}
          {tile(t("Marks"), n(event.marks))}
        </div>
      )}
      {event.tables.length > 0 && (
        <div className={`${card} scroll-thin overflow-x-auto overflow-y-hidden`}>
          <table className="w-full border-collapse tabular-nums">
            <thead>
              <tr className="text-right text-muted">
                <th className="px-2.5 py-1.5 text-left font-medium">{t("Table")}</th>
                <th className="px-2 py-1.5 font-medium">{t("Natural")}</th>
                {counted.map(([h]) => (
                  <th key={h} className="px-2 py-1.5 font-medium">
                    {t(h)}
                  </th>
                ))}
              </tr>
            </thead>
            <tbody>
              {event.tables.map((table) => (
                <tr key={table.name} className="border-t border-line text-right">
                  <td className="px-2.5 py-1.5 text-left select-text">{table.name}</td>
                  <td className={`px-2 py-1.5 ${table.natural ? "font-semibold text-danger" : ""}`}>{table.natural ? n(table.natural) : ""}</td>
                  {counted.map(([h, key]) => (
                    <td key={h} className="px-2 py-1.5">
                      {table[key] ? n(table[key]) : ""}
                    </td>
                  ))}
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
      {statement && (
        <span className="text-muted">
          {t("This statement ran {runs} times in this session: {total} in all, {each} on average.", {
            runs: n(statement.runs),
            total: took(statement.ms),
            each: took(statement.ms / Math.max(1, statement.runs)),
          })}
        </span>
      )}
    </div>
  );
}

function Live(props: { session: MonitorSession; registration: string; selected: number | null; select(seq: number | null): void }) {
  const { session, selected } = props;
  const [filter, setFilter] = useState<TraceFilter>({ group: "", attachment: null, minMs: 0, text: "" });
  const [typed, setTyped] = useState({ text: "", minMs: "", attachment: "" });
  const [nest, setNest] = useState(true);
  const [follow, setFollow] = useState(session.tick.standing === "recording" && selected === null);
  // Whether the selection was made on a row here, and not by coming from
  // a statement or a finding: only that one has to be brought into view
  const clicked = useRef(false);
  const [matching, setMatching] = useState(0);
  const [, redraw] = useState(0);
  const pages = useRef(new Map<number, TraceRow[]>());
  const asked = useRef(new Set<number>());
  const generation = useRef(0);
  const scroll = useRef<HTMLDivElement>(null);

  // What is typed becomes the filter once the typing pauses
  useEffect(() => {
    const timer = setTimeout(() => {
      const attachment = Number(typed.attachment.replace(/\D/g, ""));
      setFilter((f) => ({ ...f, text: typed.text, minMs: Math.max(0, Number(typed.minMs) || 0), attachment: attachment || null }));
    }, 250);
    return () => clearTimeout(timer);
  }, [typed]);

  const load = (page: number, again = false) => {
    if (asked.current.has(page) && !again) return;
    asked.current.add(page);
    const mine = generation.current;
    void api.tracePage(session.run, filter, page * PAGE, PAGE).then(
      (found) => {
        if (mine !== generation.current) return;
        pages.current.set(page, found.rows);
        setMatching(found.matching);
        redraw((v) => v + 1);
      },
      () => asked.current.delete(page),
    );
  };

  // Another filter, another session, or the oldest events let go: the
  // rows' places are all different
  useEffect(() => {
    generation.current += 1;
    pages.current.clear();
    asked.current.clear();
    setMatching(0);
    load(0);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [filter, session.run, session.tick.first]);

  // More events: the last page has grown, or there is one after it
  useEffect(() => {
    load(Math.floor(Math.max(0, matching - 1) / PAGE), true);
    load(Math.floor(matching / PAGE), true);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [session.tick.total]);

  const rows = useVirtualizer({ count: matching, getScrollElement: () => scroll.current, estimateSize: () => ROW_H, overscan: 20 });
  useEffect(() => {
    if (follow && matching > 0) rows.scrollToIndex(matching - 1, { align: "end" });
  }, [follow, matching, rows]);

  const plain = !filter.group && filter.attachment === null && !filter.minMs && !filter.text;
  useEffect(() => {
    if (selected === null || clicked.current || !plain || matching === 0) return;
    const index = selected - session.tick.first;
    if (index >= 0 && index < matching) rows.scrollToIndex(index, { align: "center" });
    clicked.current = true;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selected, matching, plain]);

  const cell = "truncate px-2";
  return (
    <div className="flex h-full flex-col">
      <div className="flex shrink-0 flex-wrap items-center gap-3 border-b border-line px-2 py-1.5">
        <label className="flex items-center gap-1.5 text-muted">
          {t("Events")}
          <select className={selectClass} value={filter.group} onChange={(e) => setFilter({ ...filter, group: e.target.value })}>
            {groups.map(([id, label]) => (
              <option key={id} value={id}>
                {t(label)}
              </option>
            ))}
          </select>
        </label>
        <label className="flex items-center gap-1.5 text-muted">
          {t("Connection")}
          <input className={`${inputClass} w-20`} placeholder={t("any")} value={typed.attachment} onChange={(e) => setTyped({ ...typed, attachment: e.target.value })} />
        </label>
        <label className="flex items-center gap-1.5 text-muted">
          {t("Took at least")}
          <input className={`${inputClass} w-16`} placeholder="0" inputMode="numeric" value={typed.minMs} onChange={(e) => setTyped({ ...typed, minMs: e.target.value })} />
          ms
        </label>
        <label className="flex min-w-40 flex-1 items-center gap-1.5 text-muted">
          {t("Text")}
          <input
            className={`${inputClass} min-w-0 flex-1`}
            placeholder={t("SQL, object or parameter")}
            value={typed.text}
            onChange={(e) => setTyped({ ...typed, text: e.target.value })}
          />
        </label>
        <label className="flex items-center gap-1.5">
          <input type="checkbox" className="accent-[var(--accent)]" checked={nest} onChange={(e) => setNest(e.target.checked)} />
          {t("Nest by call")}
        </label>
        <label className="flex items-center gap-1.5">
          <input type="checkbox" className="accent-[var(--accent)]" checked={follow} onChange={(e) => setFollow(e.target.checked)} />
          {t("Follow")}
        </label>
        <span className="text-faint tabular-nums">{t("{n} events", { n: n(matching) })}</span>
      </div>
      <div className="flex min-h-0 flex-1">
        <div className="flex min-w-0 flex-[3] flex-col border-r border-line">
          <div className="grid shrink-0 grid-cols-[124px_140px_minmax(0,1fr)_72px_84px] border-b border-line bg-panel-2 py-1 text-muted">
            <span className={cell}>{t("Time")}</span>
            <span className={cell}>{t("Event")}</span>
            <span className={cell}>{t("What")}</span>
            <span className={`${cell} text-right`}>{t("Took")}</span>
            <span className={`${cell} text-right`}>{t("Fetches")}</span>
          </div>
          <div
            ref={scroll}
            className="scroll-thin min-h-0 flex-1 overflow-auto"
            onWheel={(e) => e.deltaY < 0 && follow && setFollow(false)}
          >
            {matching === 0 && <Empty>{session.tick.total === 0 ? t("No events yet.") : t("No event matches.")}</Empty>}
            <div style={{ height: rows.getTotalSize(), position: "relative" }}>
              {rows.getVirtualItems().map((item) => {
                const page = Math.floor(item.index / PAGE);
                const row = pages.current.get(page)?.[item.index % PAGE];
                if (!row) {
                  load(page);
                  return null;
                }
                const slow = row.ms !== null && row.ms >= 100;
                return (
                  <div
                    key={item.index}
                    role="row"
                    aria-selected={row.seq === selected}
                    onClick={() => {
                      clicked.current = true;
                      props.select(row.seq);
                      setFollow(false);
                    }}
                    className={`absolute left-0 grid w-full cursor-default grid-cols-[124px_140px_minmax(0,1fr)_72px_84px] items-center tabular-nums ${
                      row.seq === selected ? "bg-selected" : "hover:bg-hover"
                    }`}
                    style={{ top: item.start, height: ROW_H }}
                  >
                    <span className={`${cell} ${mono} text-muted`}>{row.at.slice(11)}</span>
                    <span className={`${cell} ${row.failed ? "text-danger" : "text-muted"}`}>{kindName(row.kind)}</span>
                    <span className={`${cell} ${mono}`} style={{ paddingLeft: 8 + (nest ? Math.min(row.depth, 12) * 16 : 0) }}>
                      {row.what}
                    </span>
                    <span className={`${cell} text-right ${slow ? "font-semibold text-danger" : ""}`}>{row.ms !== null ? took(row.ms) : ""}</span>
                    <span className={`${cell} text-right`}>{row.fetches ? n(row.fetches) : ""}</span>
                  </div>
                );
              })}
            </div>
          </div>
        </div>
        <div className="scroll-thin min-w-0 flex-[2] overflow-auto bg-bg">
          <Detail session={session} seq={selected} registration={props.registration} />
        </div>
      </div>
    </div>
  );
}

// ── Statements, memory, recorded sessions ───────────────────────────────

function Statements({ session, open }: { session: MonitorSession; open(seq: number): void }) {
  const [by, setBy] = useState("time");
  const top = useTop(session, by, false, 500);
  return (
    <div className="flex h-full flex-col">
      <div className="flex shrink-0 items-center gap-3 border-b border-line px-2 py-1.5">
        <OrderSelect by={by} setBy={setBy} memory />
        <span className="text-faint">{t("Click a statement to see its slowest run.")}</span>
      </div>
      <div className="scroll-thin min-h-0 flex-1 overflow-auto">
        <StatementTable rows={top} by={by} memory open={open} />
      </div>
    </div>
  );
}

function MemoryTab({ session, open }: { session: MonitorSession; open(seq: number): void }) {
  const [by, setBy] = useState("memory");
  const sorting = useTop(session, by, true, 200);
  const [memory, setMemory] = useState<TraceMemory | null>(null);
  const recording = session.tick.standing === "recording";
  useEffect(() => {
    let live = true;
    const read = () =>
      void api.traceMemory(session.run).then(
        (m) => live && setMemory(m),
        () => {},
      );
    read();
    const timer = recording ? setInterval(read, 5000) : undefined;
    return () => {
      live = false;
      clearInterval(timer);
    };
  }, [session.run, recording]);

  const who = (m: { attachment: number; user: string | null; address: string | null }) =>
    [`ATT_${m.attachment}`, m.user, m.address].filter(Boolean).join(" · ");
  const largest = Math.max(1, ...(memory?.attachments.map((a) => a.used) ?? []));
  return (
    <div className="scroll-thin flex h-full flex-col gap-3 overflow-auto bg-bg p-3">
      <div className="grid grid-cols-[repeat(auto-fit,minmax(200px,1fr))] gap-3">
        {(
          [
            [t("Statements that sorted"), n(sorting.length), t("{n} running a statement now", { n: n(memory?.statements.length ?? 0) })],
            [t("Largest statement memory"), sorting.some((s) => s.memory > 0) ? bytes(Math.max(...sorting.map((s) => s.memory))) : "—", t("seen while it ran")],
            [t("Database memory in use"), memory ? bytes(memory.used) : "—", memory ? t("peak {peak}", { peak: bytes(memory.peak) }) : t("not read in this session")],
          ] as const
        ).map(([label, value, note]) => (
          <div key={label} className={`${card} px-3 py-2.5`}>
            <div className="text-muted">{label}</div>
            <div className="text-[22px] font-semibold tabular-nums">{value}</div>
            <div className="text-muted">{note}</div>
          </div>
        ))}
      </div>
      <div className={card}>
        <div className="flex flex-wrap items-center gap-2.5 border-b border-line px-3 py-2">
          <span className="font-semibold">{t("Statements with a sort in the plan")}</span>
          <span className="flex-1" />
          <OrderSelect by={by} setBy={setBy} memory />
        </div>
        <StatementTable rows={sorting} by={by} memory none={t("No statement with a sort in its plan finished in this session.")} open={open} />
        <div className="border-t border-line px-3 py-2 text-muted">
          {t(
            "Peak memory is the statement's own memory in MON$MEMORY_USAGE, read every 5 seconds while it runs: a statement faster than that may never be caught. Firebird doesn't report how much of a sort went to temporary files.",
          )}
        </div>
      </div>
      {memory && memory.statements.length > 0 && (
        <div className={card}>
          <div className="border-b border-line px-3 py-2 font-semibold">{t("Running now")}</div>
          {memory.statements.map((s) => (
            <div key={s.id} className="flex flex-wrap items-center gap-2.5 border-t border-line px-3 py-1.5 first:border-t-0">
              <span className={`${mono} min-w-0 flex-[1_1_300px] truncate`}>{s.sql}</span>
              <span className="text-muted">{who(s)}</span>
              <span className="w-20 text-right font-semibold tabular-nums">{bytes(s.used)}</span>
              <span className="w-28 text-right text-muted tabular-nums">{t("peak {peak}", { peak: bytes(s.peak) })}</span>
            </div>
          ))}
        </div>
      )}
      {memory && (
        <div className={card}>
          <div className="border-b border-line px-3 py-2 font-semibold">{t("Memory by connection")}</div>
          {memory.attachments.slice(0, 30).map((a) => (
            <div key={a.id} className="flex flex-wrap items-center gap-2.5 border-t border-line px-3 py-1.5 first:border-t-0">
              <span className="w-80 shrink-0 truncate" title={who(a)}>
                {who(a)}
              </span>
              <div aria-hidden className="h-2 min-w-40 flex-1 rounded-sm bg-panel-2">
                <div className="h-2 rounded-sm bg-[var(--chart-1)]" style={{ width: `${Math.max(1, (a.used / largest) * 100)}%` }} />
              </div>
              <span className="w-20 text-right font-semibold tabular-nums">{bytes(a.used)}</span>
              <span className="w-28 text-right text-muted tabular-nums">{t("peak {peak}", { peak: bytes(a.peak) })}</span>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

function Sessions({ panel, session }: { panel: AdminPanel; session?: MonitorSession }) {
  const [logs, setLogs] = useState<TraceRecorded[] | null>(null);
  const standing = session?.tick.standing;
  useEffect(() => {
    let live = true;
    void api.traceSessions(panel.registration).then(
      (found) => live && setLogs(found),
      () => live && setLogs([]),
    );
    return () => {
      live = false;
    };
  }, [panel.registration, standing]);
  if (!logs) return <Empty>{t("Loading…")}</Empty>;
  if (logs.length === 0) return <Empty>{t("No session was recorded for this database yet.")}</Empty>;
  return (
    <div className="scroll-thin h-full overflow-auto">
      {logs.map((log) => (
        <div key={log.path} className="flex flex-wrap items-center gap-3 border-b border-line px-3 py-1.5">
          <span className="w-44 font-semibold tabular-nums">{log.name.replace("_", " ")}</span>
          <span className="w-20 text-right text-muted tabular-nums">{bytes(log.bytes)}</span>
          <span className={`${mono} min-w-0 flex-1 truncate text-muted select-text`} title={log.path}>
            {log.path}
          </span>
          <TextButton icon="copy" label={t("Copy Path")} onClick={() => navigator.clipboard.writeText(log.path)} />
          <TextButton
            icon="open"
            label={session?.tick.log === log.path ? t("Open here") : t("Open")}
            disabled={standing === "recording" || session?.tick.log === log.path}
            onClick={() => openRecorded(panel.id, log.path)}
          />
        </div>
      ))}
    </div>
  );
}

// ── The panel ───────────────────────────────────────────────────────────

const thresholds: [number, string][] = [
  [0, "Everything"],
  [10, "Slower than 10 ms"],
  [100, "Slower than 100 ms"],
  [1000, "Slower than 1 s"],
];

export function ActivityMonitor({ panel, grids }: { panel: AdminPanel; grids: ReactNode }) {
  const session = useMonitor((s) => s.sessions[panel.id]);
  const error = useMonitor((s) => s.errors[panel.id]);
  const [tab, setTab] = useState<Tab>("overview");
  const [selected, setSelected] = useState<number | null>(null);
  const [options, setOptions] = useState({ thresholdMs: 0, wholeServer: false, explainedPlans: true });
  const [starting, setStarting] = useState(false);
  const [, beat] = useState(0);
  const recording = session?.tick.standing === "recording";
  // Firebird 2.5 has no plan but the short one
  const trees = useStore((s) => (s.servers[panel.registration]?.major ?? 0) >= 3);

  // The clock of a recording
  useEffect(() => {
    if (!recording) return;
    const timer = setInterval(() => beat((v) => v + 1), 1000);
    return () => clearInterval(timer);
  }, [recording]);

  useEffect(() => setSelected(null), [session?.run]);

  const start = async () => {
    setStarting(true);
    await startMonitor(panel.id, panel.registration, options);
    setStarting(false);
  };
  const open = (seq: number) => {
    setSelected(seq);
    setTab("live");
  };
  const elapsed = session?.started ? Math.floor(((session.ended ?? Date.now()) - session.started) / 1000) : null;
  const nothing = (
    <div className="flex h-full flex-col items-start gap-2 p-4">
      <p className="max-w-[60ch]">
        {t(
          "Recording follows the server step by step: every statement, procedure, trigger and transaction is written down as it runs, with its plan and what it cost, and saved to a log file.",
        )}
      </p>
      <p className="max-w-[60ch] text-muted">
        {t(
          "The server sees everything only for SYSDBA and administrators; other users see their own connections. Recording everything adds load to a busy server: record only what is slow to keep it light.",
        )}
      </p>
      <TextButton icon="run" accent label={t("Start Recording")} disabled={starting} onClick={start} />
    </div>
  );

  let pane: ReactNode;
  if (tab === "connections") pane = grids;
  else if (tab === "sessions") pane = <Sessions panel={panel} session={session} />;
  else if (!session) pane = nothing;
  else if (tab === "overview") pane = <Overview session={session} open={open} />;
  else if (tab === "live") pane = <Live session={session} registration={panel.registration} selected={selected} select={setSelected} />;
  else if (tab === "statements") pane = <Statements session={session} open={open} />;
  else pane = <MemoryTab session={session} open={open} />;

  return (
    <div className="flex h-full flex-col bg-panel">
      <div className="flex min-h-8 shrink-0 flex-wrap items-center gap-x-3 gap-y-1 border-b border-line bg-panel px-2 py-1">
        {session ? (
          <span className={`flex items-center gap-1.5 font-semibold tabular-nums ${recording || session.tick.standing === "failed" ? "text-danger" : ""}`}>
            {recording && <span aria-hidden className="size-[9px] rounded-full bg-danger" />}
            {t(standings[session.tick.standing])}
            {elapsed !== null && ` ${clock(elapsed)}`}
          </span>
        ) : (
          <span className="text-muted">{t("Not recording")}</span>
        )}
        {recording ? (
          <TextButton icon="stop" label={t("Stop")} onClick={() => stopMonitor(panel.id)} />
        ) : (
          <TextButton icon="run" accent label={session ? t("Record Again") : t("Start Recording")} disabled={starting} onClick={start} />
        )}
        <label className="flex items-center gap-1.5 text-muted">
          {t("Scope")}
          <select
            className={selectClass}
            disabled={recording}
            value={options.wholeServer ? "server" : "database"}
            onChange={(e) => setOptions({ ...options, wholeServer: e.target.value === "server" })}
          >
            <option value="database">{t("This database")}</option>
            <option value="server">{t("Whole server")}</option>
          </select>
        </label>
        <label className="flex items-center gap-1.5 text-muted">
          {t("Record")}
          <select
            className={selectClass}
            disabled={recording}
            value={options.thresholdMs}
            onChange={(e) => setOptions({ ...options, thresholdMs: Number(e.target.value) })}
          >
            {thresholds.map(([ms, label]) => (
              <option key={ms} value={ms}>
                {t(label)}
              </option>
            ))}
          </select>
        </label>
        <label className="flex items-center gap-1.5 text-muted" title={trees ? undefined : t("This server is older than Firebird 3, which is where plans as trees begin.")}>
          <input
            type="checkbox"
            className="accent-[var(--accent)]"
            disabled={recording || !trees}
            checked={options.explainedPlans && trees}
            onChange={(e) => setOptions({ ...options, explainedPlans: e.target.checked })}
          />
          {t("Plans as trees")}
        </label>
        <span className="flex-1" />
        {session && (
          <>
            <span className="text-muted">
              {t("Events")} <b className="text-fg tabular-nums">{n(session.tick.total)}</b>
            </span>
            <span
              className="text-muted"
              title={t("Times the server dropped events because its buffer filled before it was read. What was dropped is not in the log.")}
            >
              {t("Gaps")} <b className={`tabular-nums ${session.tick.gaps ? "text-danger" : "text-ok"}`}>{n(session.tick.gaps)}</b>
            </span>
            <span className={`${mono} text-muted select-text`} title={session.tick.log}>
              {session.tick.log.split(/[\\/]/).slice(-1)[0]}
            </span>
            <IconButton icon="copy" label={t("Copy the Log's Path")} onClick={() => navigator.clipboard.writeText(session.tick.log)} />
          </>
        )}
        <IconButton
          icon="folder"
          label={t("Open the Folder of This Database's Logs")}
          onClick={() => api.traceFolder(panel.registration).catch((e) => useStore.getState().setNotice(errorText(e)))}
        />
      </div>
      {(error || session?.tick.error) && (
        <p className="shrink-0 border-b border-line px-2 py-1.5 whitespace-pre-wrap text-danger select-text" role="alert">
          {error || session?.tick.error}
        </p>
      )}
      {session && session.tick.notices.some((notice) => !notice.startsWith("Trace session ID")) && (
        <p className="shrink-0 border-b border-line px-2 py-1.5 whitespace-pre-wrap text-warn select-text">
          {session.tick.notices.filter((notice) => !notice.startsWith("Trace session ID")).join("\n")}
        </p>
      )}
      <div className="flex h-7 shrink-0 items-stretch border-b border-line bg-panel-2" role="tablist">
        {tabs.map(([id, label]) => (
          <button
            key={id}
            role="tab"
            aria-selected={tab === id}
            onClick={() => setTab(id)}
            className={`border-r border-line px-3 ${tab === id ? "bg-panel text-fg shadow-[inset_0_-2px_0_var(--accent)]" : "text-muted hover:bg-hover"}`}
          >
            {t(label)}
          </button>
        ))}
      </div>
      <div className="min-h-0 flex-1">{pane}</div>
    </div>
  );
}

/** A toolbar button that is its icon alone */
function IconButton(props: { icon: string; label: string; onClick(): unknown }) {
  return (
    <button
      type="button"
      title={props.label}
      aria-label={props.label}
      onClick={() => void props.onClick()}
      className="grid size-6 shrink-0 place-items-center rounded text-fg hover:bg-hover"
    >
      <Icon name={props.icon} size={13} />
    </button>
  );
}
