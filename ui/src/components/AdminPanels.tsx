// The administration tools' tabs: the activity monitor and the users, as
// the grids the backend draws, and a service's output as it is written.
import { useVirtualizer } from "@tanstack/react-virtual";
import type { IDockviewPanelProps } from "dockview-react";
import { useEffect, useRef, useState } from "react";
import { cancelService, linesOf, startAdminAction, useAdmin, useServices } from "../admin";
import { t } from "../i18n";
import { useStore } from "../store";
import { Section, TextButton } from "./ObjectViewer";

/** How often the monitor reads again by itself, in seconds; 0 is never */
const intervals = [0, 2, 5, 10, 30];

export function AdminView({ params, api: dock }: IDockviewPanelProps<{ id: string }>) {
  const panel = useAdmin((s) => s.panels[params.id]);
  const connected = useStore((s) => (panel ? !!s.servers[panel.registration] : false));
  const [every, setEvery] = useState(0);
  // The row the cursor is on: [section, row]
  const [cursor, setCursor] = useState<[number, number]>([0, 0]);
  const dialog = useStore((s) => s.dialog !== null);

  // What it shows is the connection's: without one, or after a restart of
  // the app, there is nothing to show
  useEffect(() => {
    if (!panel || !connected) dock.close();
  }, [panel, connected, dock]);

  // Reading again moves nothing under a dialog that is about a row
  useEffect(() => {
    if (!every || dialog) return;
    const timer = setInterval(() => void useAdmin.getState().load(params.id), every * 1000);
    return () => clearInterval(timer);
  }, [every, dialog, params.id]);

  if (!panel) return <div className="h-full bg-panel" />;
  const monitor = panel.view === "monitor";
  const { sections } = panel;

  return (
    <div className="flex h-full flex-col bg-panel">
      <div className="flex h-8 shrink-0 items-center gap-1 border-b border-line bg-panel-2 px-2">
        <TextButton icon="refresh" label={t("Refresh")} disabled={panel.loading} onClick={() => useAdmin.getState().load(panel.id)} />
        {monitor && (
          <>
            <label className="ml-2 flex items-center gap-1.5 text-muted">
              {t("Every")}
              <select
                className="h-6 rounded border border-line bg-panel px-1 text-fg"
                value={every}
                onChange={(e) => setEvery(Number(e.target.value))}
              >
                {intervals.map((seconds) => (
                  <option key={seconds} value={seconds}>
                    {seconds === 0 ? t("never") : t("{n} s", { n: seconds })}
                  </option>
                ))}
              </select>
            </label>
            <label className="ml-3 flex items-center gap-1.5 text-muted">
              <input
                type="checkbox"
                className="accent-[var(--accent)]"
                checked={panel.idle}
                onChange={(e) => useAdmin.getState().setIdle(panel.id, e.target.checked)}
              />
              {t("Idle statements too")}
            </label>
          </>
        )}
        <span className="ml-auto truncate text-faint tabular-nums">
          {panel.loading ? t("Reading…") : panel.at ? t("Read at {time}", { time: panel.at.toLocaleTimeString() }) : ""}
        </span>
      </div>
      {panel.error && (
        <p className="shrink-0 border-b border-line px-2 py-1.5 whitespace-pre-wrap text-danger select-text" role="alert">
          {panel.error}
        </p>
      )}
      <div className="scroll-thin min-h-0 flex-1 overflow-auto">
        {!sections && !panel.error && <p className="px-2 py-1.5 text-muted">{t("Loading…")}</p>}
        {sections?.map((section, s) => (
          <Section
            key={s}
            section={section}
            current={cursor[0] === s ? cursor[1] : -1}
            onCurrent={(row) => setCursor([s, row])}
            open={() => {}}
            act={(action, key) => void startAdminAction(panel, action, key)}
            sticky={sections.length === 1}
          />
        ))}
      </div>
    </div>
  );
}

const LINE_H = 18;

const states = {
  running: "Running…",
  finished: "Finished",
  cancelled: "Stopped listening: the server may still be at it",
  failed: "Failed",
} as const;

function elapsed(from: Date, to: Date): string {
  const seconds = Math.max(0, Math.round((to.getTime() - from.getTime()) / 1000));
  return seconds < 60 ? `${seconds} s` : `${Math.floor(seconds / 60)} min ${seconds % 60} s`;
}

export function ServiceOutput({ params }: IDockviewPanelProps<{ id: string }>) {
  const run = useServices((s) => s.runs[params.id]);
  const scroll = useRef<HTMLDivElement>(null);
  // Follows the last line until the user scrolls away from it
  const following = useRef(true);
  const [, tick] = useState(0);
  const lines = linesOf(params.id);
  const count = run?.count ?? 0;

  const rows = useVirtualizer({
    count,
    getScrollElement: () => scroll.current,
    estimateSize: () => LINE_H,
    overscan: 30,
  });

  useEffect(() => {
    if (following.current && count > 0) rows.scrollToIndex(count - 1, { align: "end" });
  }, [count, rows]);

  // The clock of a service that is running and silent
  useEffect(() => {
    if (run?.state !== "running") return;
    const timer = setInterval(() => tick((n) => n + 1), 1000);
    return () => clearInterval(timer);
  }, [run?.state]);

  // After a restart of the app there is no run behind the tab
  if (!run) return <div className="h-full bg-panel px-2 py-1.5 text-muted">{t("This output is no longer kept.")}</div>;

  const running = run.state === "running";
  return (
    <div className="flex h-full flex-col bg-panel">
      <div className="flex h-8 shrink-0 items-center gap-2 border-b border-line bg-panel-2 px-2">
        <span className={run.state === "failed" ? "font-semibold text-danger" : running ? "font-semibold" : "font-semibold text-ok"}>
          {t(states[run.state])}
        </span>
        <span className="text-faint tabular-nums">
          {elapsed(run.started, run.ended ?? new Date())} · {t("{n} lines", { n: count.toLocaleString() })}
        </span>
        <div className="ml-auto flex items-center gap-1">
          <TextButton icon="copy" label={t("Copy All")} disabled={count === 0} onClick={() => navigator.clipboard.writeText(lines.join("\n"))} />
          <TextButton icon="rollback" label={t("Stop")} disabled={!running} onClick={() => cancelService(run.id)} />
        </div>
      </div>
      {run.error && (
        <p className="shrink-0 border-b border-line px-2 py-1.5 whitespace-pre-wrap text-danger select-text" role="alert">
          {run.error}
        </p>
      )}
      <div
        ref={scroll}
        className="scroll-thin min-h-0 flex-1 overflow-auto font-mono text-[12px] select-text"
        onScroll={(e) => {
          const el = e.currentTarget;
          following.current = el.scrollHeight - el.scrollTop - el.clientHeight < LINE_H * 2;
        }}
      >
        {count === 0 && !running && !run.error && <p className="px-2 py-1.5 font-sans text-muted">{t("It wrote nothing.")}</p>}
        <div style={{ height: rows.getTotalSize(), position: "relative" }}>
          {rows.getVirtualItems().map((item) => (
            <div
              key={item.index}
              className="absolute left-0 px-2 whitespace-pre"
              style={{ top: item.start, height: LINE_H, lineHeight: `${LINE_H}px` }}
            >
              {lines[item.index]}
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}
