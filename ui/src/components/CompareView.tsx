// A comparison's tab: how many rows are the same, which are not and in
// what, and the script that would make the target the source.
import { useVirtualizer } from "@tanstack/react-virtual";
import type { IDockviewPanelProps } from "dockview-react";
import { useMemo, useRef, useState } from "react";
import type { Comparison, DiffKind } from "../api";
import { cancelComparison, runComparison, useComparisons, type ComparePanel } from "../compare";
import { openEditor } from "../dock";
import { t } from "../i18n";
import { useStore } from "../store";
import { TextButton } from "./ObjectViewer";

const ROW_H = 22;

const kinds: Record<DiffKind, { label: string; color: string }> = {
  different: { label: "Different", color: "text-accent" },
  only_source: { label: "Only in source", color: "text-ok" },
  only_target: { label: "Only in target", color: "text-danger" },
};

const phases: Record<string, string> = {
  target: "Reading the target: {n} rows",
  source: "Reading the source: {n} rows",
  details: "Reading what differs: {n} rows",
};

interface Line {
  kind: DiffKind;
  /** The first line of its row: the only one that names it */
  first: boolean;
  key: string;
  column: string;
  source: string | null;
  target: string | null;
}

const NULL = "NULL";
const shown = (value: string | null) => value ?? NULL;

/** One line for each column that differs; one for a row only one side has */
function lines(result: Comparison, only: DiffKind | "all"): Line[] {
  const out: Line[] = [];
  for (const row of result.rows) {
    if (only !== "all" && row.kind !== only) continue;
    const key = row.key.map(shown).join(" · ");
    if (row.kind === "different") {
      row.changes.forEach((c, i) => out.push({ kind: row.kind, first: i === 0, key, column: c.column, source: shown(c.source), target: shown(c.target) }));
    } else {
      const values = row.changes.map((c) => `${c.column}=${shown(c.source)}`).join(", ");
      out.push({
        kind: row.kind,
        first: true,
        key,
        column: "",
        source: row.kind === "only_source" ? values || t("(the row)") : null,
        target: row.kind === "only_target" ? t("(the row)") : null,
      });
    }
  }
  return out;
}

function script(panel: ComparePanel, result: Comparison, parts: Record<"deletes" | "updates" | "inserts", boolean>): string {
  const name = (id: string) => useStore.getState().registrations.find((r) => r.id === id)?.name ?? "";
  const body = (["deletes", "updates", "inserts"] as const).filter((p) => parts[p]).map((p) => result[p]).filter(Boolean);
  const header = [
    `/* Makes ${panel.targetTable} (${name(panel.target)}) like ${panel.sourceTable} (${name(panel.source)}),`,
    `   as they were when compared. Read it, then run it as a script.`,
    ...(result.truncated ? [`   NOT COMPLETE: there are more differences than were kept.`] : []),
    ` */`,
  ];
  return `${header.join("\n")}\n${body.join("\n")}`;
}

export function CompareView({ params }: IDockviewPanelProps<{ id: string }>) {
  const panel = useComparisons((s) => s.panels[params.id]);
  const targetConnected = useStore((s) => (panel ? !!s.servers[panel.target] : false));
  const [only, setOnly] = useState<DiffKind | "all">("all");
  const [parts, setParts] = useState({ deletes: true, updates: true, inserts: true });
  const scroll = useRef<HTMLDivElement>(null);
  const result = panel?.result;
  const list = useMemo(() => (result ? lines(result, only) : []), [result, only]);
  const rows = useVirtualizer({ count: list.length, getScrollElement: () => scroll.current, estimateSize: () => ROW_H, overscan: 20 });

  // After a restart of the app there is no comparison behind the tab
  if (!panel) return <div className="h-full bg-panel px-2 py-1.5 text-muted">{t("This comparison is no longer kept.")}</div>;

  const running = panel.state === "running";
  const differences = result ? result.different + result.onlySource + result.onlyTarget : 0;
  const names = useStore.getState().registrations;
  const name = (id: string) => names.find((r) => r.id === id)?.name ?? "";
  const hasScript = !!result && (["deletes", "updates", "inserts"] as const).some((p) => parts[p] && result[p]);
  const grid = "grid grid-cols-[8rem_minmax(8rem,14rem)_minmax(7rem,12rem)_minmax(10rem,1fr)_minmax(10rem,1fr)]";

  return (
    <div className="flex h-full flex-col bg-panel">
      <div className="flex min-h-8 shrink-0 flex-wrap items-center gap-x-3 gap-y-1 border-b border-line bg-panel-2 px-2 py-1">
        <span className="font-semibold">
          {panel.sourceTable} <span className="font-normal text-muted">({name(panel.source)})</span> → {panel.targetTable}{" "}
          <span className="font-normal text-muted">({name(panel.target)})</span>
        </span>
        {running && (
          <span className="text-muted tabular-nums">
            {panel.progress ? t(phases[panel.progress.phase] ?? "{n} rows", { n: panel.progress.rows.toLocaleString() }) : t("Comparing…")}
          </span>
        )}
        {result && !running && (
          <span className="text-muted tabular-nums">
            {result.cancelled
              ? t("Stopped before the end.")
              : t("{same} the same · {different} different · {source} only in source · {target} only in target", {
                  same: result.same.toLocaleString(),
                  different: result.different.toLocaleString(),
                  source: result.onlySource.toLocaleString(),
                  target: result.onlyTarget.toLocaleString(),
                })}
          </span>
        )}
        <div className="ml-auto flex items-center gap-1">
          {running ? (
            <TextButton icon="rollback" label={t("Stop")} onClick={() => cancelComparison(panel.id)} />
          ) : (
            <TextButton icon="refresh" label={t("Compare Again")} onClick={() => runComparison(panel.id)} />
          )}
        </div>
      </div>
      {panel.error && (
        <p className="shrink-0 border-b border-line px-2 py-1.5 whitespace-pre-wrap text-danger select-text" role="alert">
          {panel.error}
        </p>
      )}
      {result && !running && (
        <div className="flex min-h-8 shrink-0 flex-wrap items-center gap-x-3 gap-y-1 border-b border-line px-2 py-1">
          <label className="flex items-center gap-1.5 text-muted">
            {t("Show")}
            <select className="h-6 rounded border border-line bg-panel px-1 text-fg" value={only} onChange={(e) => setOnly(e.target.value as DiffKind | "all")}>
              <option value="all">{t("every difference")}</option>
              <option value="different">{t("different rows")}</option>
              <option value="only_source">{t("rows only in the source")}</option>
              <option value="only_target">{t("rows only in the target")}</option>
            </select>
          </label>
          <span className="ml-auto text-muted">{t("Script:")}</span>
          {(
            [
              ["deletes", "Delete", result.onlyTarget],
              ["updates", "Update", result.different],
              ["inserts", "Insert", result.onlySource],
            ] as const
          ).map(([part, label, count]) => (
            <label key={part} className="flex items-center gap-1.5">
              <input
                type="checkbox"
                className="accent-[var(--accent)]"
                checked={parts[part]}
                disabled={count === 0}
                onChange={(e) => setParts((p) => ({ ...p, [part]: e.target.checked }))}
              />
              {t(label)}
            </label>
          ))}
          <TextButton
            icon="sql"
            label={t("To SQL Editor")}
            disabled={!hasScript || !targetConnected}
            onClick={() => openEditor(panel.target, script(panel, result, parts))}
          />
        </div>
      )}
      {result && (result.notes.length > 0 || result.truncated) && !running && (
        <div className="shrink-0 border-b border-line px-2 py-1 text-[11.5px] text-muted select-text">
          {result.truncated && (
            <p className="text-danger">
              {t("Only the first {n} differences are listed and scripted.", { n: result.rows.length.toLocaleString() })}
            </p>
          )}
          {result.notes.map((note) => (
            <p key={note}>{note}</p>
          ))}
        </div>
      )}
      <div ref={scroll} className="scroll-thin min-h-0 flex-1 overflow-auto select-text">
        {result && !running && differences === 0 && !result.cancelled && (
          <p className="px-2 py-1.5 text-ok">{t("The two tables have the same rows, in the columns compared.")}</p>
        )}
        {list.length > 0 && (
          <div className="min-w-[52rem]">
            <div className={`${grid} sticky top-0 z-10 border-b border-line bg-panel-2 font-semibold text-muted`} style={{ height: ROW_H }}>
              {[t("Difference"), result?.key.join(" · ") ?? t("Key"), t("Column"), t("Source"), t("Target")].map((label, i) => (
                <div key={i} className="truncate border-r border-line px-2 leading-[22px]">
                  {label}
                </div>
              ))}
            </div>
            <div style={{ height: rows.getTotalSize(), position: "relative" }}>
              {rows.getVirtualItems().map((item) => {
                const line = list[item.index];
                const cell = "truncate border-r border-b border-line/60 px-2 leading-[22px]";
                return (
                  <div key={item.index} className={`${grid} absolute inset-x-0 hover:bg-hover`} style={{ top: item.start, height: ROW_H }}>
                    <div className={`${cell} ${kinds[line.kind].color}`}>{line.first ? t(kinds[line.kind].label) : ""}</div>
                    <div className={`${cell} tabular-nums`}>{line.first ? line.key : ""}</div>
                    <div className={cell}>{line.column}</div>
                    <div className={`${cell} ${line.source === NULL || line.source === null ? "text-faint" : ""}`} title={line.source ?? undefined}>
                      {line.source ?? "—"}
                    </div>
                    <div className={`${cell} ${line.target === NULL || line.target === null ? "text-faint" : ""}`} title={line.target ?? undefined}>
                      {line.target ?? "—"}
                    </div>
                  </div>
                );
              })}
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
