// What the last statement cost, as a small dashboard: the figures that
// matter first, where the time went, what was read in each table, and
// what stands out in it said in words (perf.ts).
import { useMemo, type ReactNode } from "react";
import type { EditorTab } from "../editors";
import { t } from "../i18n";
import { summarize, type Finding, type PerfSummary } from "../perf";
import { sources } from "../sqlScope";
import { Empty } from "./Results";

const card = "rounded-md border border-line bg-panel";
const n = (value: number) => Math.round(value).toLocaleString();

/** A time as it is read at a glance: 0.5 ms, 59.6 ms, 2.19 s */
function time(ms: number): string {
  if (ms >= 1000) return `${(ms / 1000).toFixed(2)} s`;
  return `${ms >= 100 ? Math.round(ms) : ms.toFixed(1)} ms`;
}

function size(bytes: number): string {
  const gb = bytes / 1024 ** 3;
  return gb >= 1 ? `${gb.toFixed(2)} GB` : `${Math.round(bytes / 1024 ** 2).toLocaleString()} MB`;
}

const tones: Record<Finding["tone"], string> = {
  danger: "text-danger",
  warn: "text-warn",
  info: "text-accent",
  ok: "text-ok",
  muted: "text-muted",
};

function Tile(props: { label: string; value: string; note: string; tone?: string; noteTone?: string }) {
  return (
    <div className={`${card} px-3 py-2`}>
      <div className="text-muted">{props.label}</div>
      <div className={`text-[20px] leading-7 font-semibold tabular-nums ${props.tone ?? ""}`}>{props.value}</div>
      <div className={props.noteTone ?? "text-muted"}>{props.note}</div>
    </div>
  );
}

function Card({ title, legend, className, children }: { title: string; legend?: ReactNode; className: string; children: ReactNode }) {
  return (
    <section className={`${card} flex min-w-0 flex-col gap-2 px-3 py-2.5 ${className}`}>
      <div className="flex flex-wrap items-center gap-x-3.5 gap-y-1">
        <h3 className="flex-1 font-semibold">{title}</h3>
        {legend}
      </div>
      {children}
    </section>
  );
}

const Swatch = ({ color }: { color: string }) => <span aria-hidden className="size-2.5 shrink-0 rounded-sm" style={{ background: color }} />;

const PREPARE = "var(--faint)";
const EXECUTE = "var(--chart-1)";
const FETCH = "var(--chart-2)";

export function PerformancePane({ tab }: { tab: EditorTab }) {
  const outcome = tab.outcome;
  const summary = useMemo(() => {
    if (!outcome) return null;
    const { executed, result } = outcome;
    return summarize({
      prepareMs: executed.prepareMs,
      executeMs: executed.executeMs,
      fetchMs: result ? result.fetchMs : null,
      rows: result ? result.rows : null,
      performance: result ? result.performance : executed.performance,
      named: sources(outcome.sent.sql, 0).map((s) => s.relation),
    });
  }, [outcome]);
  if (!outcome || !summary) return <Empty>{t("Execute a statement to see what it cost.")}</Empty>;
  const { executed, result } = outcome;
  const perf = result ? result.performance : executed.performance;
  const { per, changed } = summary;

  const parts = [
    { label: t("Prepare"), ms: executed.prepareMs, color: PREPARE },
    { label: t("Execute"), ms: executed.executeMs, color: EXECUTE },
    ...(result ? [{ label: t("Fetch, {n} row(s)", { n: n(result.rows) }), ms: result.fetchMs, color: FETCH }] : []),
  ];
  const share = (ms: number) => (summary.totalMs > 0 ? (ms / summary.totalMs) * 100 : 0);
  const changes = [
    changed.updated && t("{n} updated", { n: n(changed.updated) }),
    changed.inserted && t("{n} inserted", { n: n(changed.inserted) }),
    changed.deleted && t("{n} deleted", { n: n(changed.deleted) }),
  ].filter(Boolean);

  return (
    <div className="scroll-thin flex h-full flex-col gap-2.5 overflow-auto p-2.5">
      <div className="grid shrink-0 grid-cols-[repeat(auto-fit,minmax(180px,1fr))] gap-2.5">
        <Tile
          label={t("Total time")}
          value={time(summary.totalMs)}
          note={result ? t("prepare, execute and fetch") : t("prepare and execute")}
        />
        {changed.total > 0 ? (
          <Tile
            label={t("Rows changed")}
            value={n(changed.total)}
            note={`${changes.join(", ")}${changed.tables > 1 ? ` · ${t("{n} tables", { n: changed.tables })}` : ""}`}
          />
        ) : result ? (
          <Tile label={t("Rows returned")} value={n(result.rows)} note={result.eof ? t("all of them fetched") : t("fetched so far: there are more")} />
        ) : (
          <Tile label={t("Rows changed")} value="0" note={t("none inserted, updated or deleted")} />
        )}
        {perf && (
          <>
            <Tile
              label={t("Records read")}
              value={n(summary.read)}
              tone={summary.findings.some((f) => f.kind === "Scan") ? "text-danger" : ""}
              noteTone={summary.findings.some((f) => f.kind === "Scan") ? "text-danger" : undefined}
              note={
                per
                  ? t(per.what === "changed" ? "{n} for each row changed" : "{n} for each row returned", { n: n(summary.read / per.rows) })
                  : t("in {n} table(s)", { n: summary.tables.length })
              }
            />
            <Tile
              label={t("Pages touched")}
              value={n(perf.fetches)}
              noteTone={perf.reads === 0 ? "text-ok" : perf.reads >= 100 ? "text-warn" : undefined}
              note={
                perf.reads === 0
                  ? t("all from the cache, none from disk")
                  : t("{n} read from disk, {share}% from the cache", { n: n(perf.reads), share: ((summary.cached ?? 0) * 100).toFixed(1) })
              }
            />
            <Tile label={t("Pages written")} value={n(perf.writes)} note={t("{n} changed in the cache", { n: n(perf.marks) })} />
          </>
        )}
      </div>

      <div className="flex shrink-0 flex-wrap items-stretch gap-2.5">
        <Card title={t("Where the time went")} className="flex-[1_1_280px]">
          <div
            role="img"
            aria-label={parts.map((p) => `${p.label} ${Math.round(share(p.ms))}%`).join(", ")}
            className="flex h-4 gap-0.5 overflow-hidden rounded-sm"
          >
            {parts.map((p) => (
              <div key={p.label} style={{ flex: `${Math.max(share(p.ms), 0.01)} 1 0`, minWidth: 3, background: p.color }} />
            ))}
          </div>
          <div className="grid grid-cols-[auto_1fr_auto_auto] items-center gap-x-2.5 gap-y-1 tabular-nums">
            {parts.map((p) => (
              <div key={p.label} className="contents">
                <Swatch color={p.color} />
                <span>{p.label}</span>
                <span className="text-right">{time(p.ms)}</span>
                <span className="text-right text-muted">{Math.round(share(p.ms))}%</span>
              </div>
            ))}
          </div>
          <div className="mt-auto grid grid-cols-[1fr_auto] gap-x-2.5 gap-y-1 border-t border-line pt-2 tabular-nums">
            {outcome.previousMs !== undefined && (
              <>
                <span className="text-muted">{t("Last run of this statement")}</span>
                <span className="text-right">{time(outcome.previousMs)}</span>
              </>
            )}
            <span className="text-muted">{t("This one, without the prepare")}</span>
            <span className="text-right">{time(summary.totalMs - executed.prepareMs)}</span>
            {per && summary.totalMs > 0 && per.what === "changed" && (
              <>
                <span className="text-muted">{t("Time for each row changed")}</span>
                <span className="text-right">{time(summary.totalMs / per.rows)}</span>
              </>
            )}
          </div>
        </Card>

        {perf ? (
          <>
            <Card
              title={t("Records read, by table")}
              className="flex-[2_1_440px]"
              legend={
                <>
                  <span className="flex items-center gap-1.5 text-muted">
                    <Swatch color={EXECUTE} />
                    {t("through an index")}
                  </span>
                  <span className="flex items-center gap-1.5 text-muted">
                    <Swatch color={FETCH} />
                    {t("without one")}
                  </span>
                </>
              }
            >
              <Tables summary={summary} />
              <p className="border-t border-line pt-2 text-muted">{cleaning(summary)}</p>
            </Card>
            <Card title={t("What stands out")} className="flex-[1_1_260px]">
              <div className="grid grid-cols-[auto_1fr] gap-x-2.5 gap-y-2">
                {summary.findings.map((f, i) => (
                  <div key={i} className="contents">
                    <span className={`font-semibold ${tones[f.tone]}`}>{t(f.kind)}</span>
                    <span className="select-text">{t(f.text, f.vars)}</span>
                  </div>
                ))}
              </div>
            </Card>
          </>
        ) : (
          <Card title={t("Records and pages")} className="flex-[3_1_440px]">
            <p className="text-muted">
              {result && !result.eof
                ? t("Fetch all the rows to see what the statement read: the server counts it for the whole of it.")
                : t("Counters are measured when a single statement runs.")}
            </p>
          </Card>
        )}
      </div>

      {perf && (
        <div className="flex shrink-0 flex-wrap gap-x-5 gap-y-1 text-muted tabular-nums">
          <span>{t("The server while it ran")}</span>
          <span>
            {t("Database memory")} <b className="font-semibold text-fg">{size(perf.currentMemory)}</b>, {t("peak {size}", { size: size(perf.maxMemory) })}
          </span>
          <span>
            {t("Page cache")} <b className="font-semibold text-fg">{t("{n} pages", { n: n(perf.buffers) })}</b>{" "}
            {t("of {kb} KB, {size}", { kb: n(perf.pageSize / 1024), size: size(perf.buffers * perf.pageSize) })}
          </span>
        </div>
      )}
    </div>
  );
}

/** What was cleaned on the way, or that nothing was */
function cleaning({ cleaned, changed }: PerfSummary): string {
  const parts = [
    cleaned.purges && t("{n} purged", { n: n(cleaned.purges) }),
    cleaned.expunges && t("{n} expunged", { n: n(cleaned.expunges) }),
    cleaned.backouts && t("{n} backed out", { n: n(cleaned.backouts) }),
  ].filter(Boolean);
  if (parts.length > 0) return t("Old record versions cleaned on the way: {parts}.", { parts: parts.join(", ") });
  return changed.total > 0
    ? t("No old record version was cleaned on the way.")
    : t("No record was inserted, updated or deleted, and no old record version was cleaned on the way.");
}

/** The tables, the one read most first; the table takes the card's height,
 *  so what is said under it stays at the card's end */
function Tables({ summary }: { summary: PerfSummary }) {
  const { tables, per, changed } = summary;
  if (tables.length === 0) return <p className="flex-1 text-muted">{t("No table was read or written.")}</p>;
  const most = Math.max(1, ...tables.map((x) => x.read));
  const last = changed.total > 0 ? t("Changed") : per ? t("Per row returned") : "";
  return (
    <div className="scroll-thin min-h-0 flex-1 overflow-x-auto">
      <div className="grid min-w-[380px] grid-cols-[minmax(100px,auto)_minmax(60px,1fr)_auto_auto_auto] items-center gap-x-3 gap-y-1.5 tabular-nums">
        <span className="text-muted">{t("Table")}</span>
        <span />
        <span className="text-right text-muted">{t("Indexed")}</span>
        <span className="text-right text-muted">{t("Non-indexed")}</span>
        <span className="text-right text-muted">{last}</span>
        {tables.map((x) => {
          const done = [
            x.updates && t("{n} updated", { n: n(x.updates) }),
            x.inserts && t("{n} inserted", { n: n(x.inserts) }),
            x.deletes && t("{n} deleted", { n: n(x.deletes) }),
          ].filter(Boolean);
          return (
            <div key={x.table} className="contents">
              <span className="font-semibold select-text" title={x.indirect ? t("Not named in the statement") : undefined}>
                {x.table}
              </span>
              <span
                role="img"
                aria-label={t("{indexed} through an index, {natural} without one", { indexed: n(x.indexedReads), natural: n(x.nonIndexedReads) })}
                className="flex h-3 overflow-hidden rounded-sm bg-hover"
              >
                {x.indexedReads > 0 && <span style={{ width: `${(x.indexedReads / most) * 100}%`, minWidth: 3, background: EXECUTE }} />}
                {x.nonIndexedReads > 0 && <span style={{ width: `${(x.nonIndexedReads / most) * 100}%`, minWidth: 3, background: FETCH }} />}
              </span>
              <span className={`text-right ${x.indexedReads ? "" : "text-faint"}`}>{n(x.indexedReads)}</span>
              <span className={`text-right ${x.nonIndexedReads ? (x.nonIndexedReads >= 1000 ? "font-semibold text-danger" : "") : "text-faint"}`}>
                {n(x.nonIndexedReads)}
              </span>
              <span className={`text-right ${changed.total > 0 && done.length === 0 ? "text-faint" : ""}`}>
                {changed.total > 0 ? done.join(", ") || t("none") : per ? (x.read / per.rows).toFixed(1) : ""}
              </span>
            </div>
          );
        })}
      </div>
    </div>
  );
}
