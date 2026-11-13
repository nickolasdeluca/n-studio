// A debugger's tab: the routine's source with the statement it is at, the
// variables as they are there, and the rows it has suspended so far.
import type { IDockviewPanelProps } from "dockview-react";
import { useEffect, useMemo, useRef } from "react";
import { alive, setDebugInput, startDebug, tellDebug, toggleBreakpoint, useDebug, type DebugPanel } from "../debug";
import { t } from "../i18n";
import { useStore } from "../store";
import { TextButton } from "./ObjectViewer";

const heading = "flex h-7 shrink-0 items-center gap-2 border-b border-line bg-panel-2 px-2 font-semibold";

const states: Record<DebugPanel["state"], string> = {
  loading: "Reading the routine…",
  idle: "Not started. What it does is done in a transaction of its own.",
  running: "Running…",
  paused: "Stopped at line {line}.",
  finished: "Finished. Its transaction is open: commit it or roll it back.",
  ended: "Ended.",
  stopped: "Stopped, and rolled back.",
  failed: "Failed, and rolled back.",
};

const kinds = { input: "in", output: "out", local: "" } as const;

function Source({ panel }: { panel: DebugPanel }) {
  const lines = useMemo(() => (panel.target?.source ?? "").split("\n"), [panel.target]);
  const current = useRef<HTMLLIElement>(null);
  const refused = useMemo(() => new Map((panel.target?.notes ?? []).map((n) => [n.line, n.what])), [panel.target]);
  useEffect(() => {
    current.current?.scrollIntoView({ block: "nearest" });
  }, [panel.line, panel.state]);
  const failed = panel.state === "failed";
  return (
    <ol className="min-h-0 flex-1 overflow-auto py-1 font-mono select-text">
      {lines.map((text, i) => {
        const line = i + 1;
        const at = panel.line !== null && line >= panel.line && line <= (panel.endLine ?? panel.line);
        const broken = panel.breakpoints.includes(line);
        return (
          <li
            key={line}
            ref={line === panel.line ? current : undefined}
            className={`flex min-w-max ${at ? (failed ? "bg-danger/20" : "bg-accent/20") : ""}`}
            aria-current={at ? "step" : undefined}
          >
            <button
              type="button"
              className="flex w-14 shrink-0 items-center justify-end gap-1.5 pr-2 text-right text-faint select-none hover:text-fg"
              title={broken ? t("Remove the breakpoint") : t("Stop here")}
              aria-pressed={broken}
              onClick={() => toggleBreakpoint(panel.id, line)}
            >
              <span className={`size-2 rounded-full ${broken ? "bg-danger" : ""}`} />
              {line}
            </button>
            <span className={`whitespace-pre pr-4 ${refused.has(line) ? "underline decoration-danger decoration-wavy" : ""}`} title={refused.get(line)}>
              {text || " "}
            </span>
          </li>
        );
      })}
    </ol>
  );
}

export function DebugView({ params }: IDockviewPanelProps<{ id: string }>) {
  const panel = useDebug((s) => s.panels[params.id]);
  const connected = useStore((s) => (panel ? !!s.servers[panel.registration] : false));

  // After a restart of the app there is no routine behind the tab
  if (!panel) return <div className="h-full bg-panel px-2 py-1.5 text-muted">{t("This debugging session is no longer kept.")}</div>;

  const { target } = panel;
  const live = alive(panel);
  const paused = panel.state === "paused";
  const finished = panel.state === "finished";
  const startable = !!target && connected && !live;
  const run = () => (paused ? tellDebug(panel.id, "continue") : startDebug(panel.id, false));
  const step = () => (paused ? tellDebug(panel.id, "step") : startDebug(panel.id, true));
  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "F9" && (paused || startable)) void run();
    else if (e.key === "F8" && (paused || startable)) void step();
    else return;
    e.preventDefault();
  };
  // Before a run, what it will be given; from then on, what it has
  const variables = panel.variables.length > 0 || live ? panel.variables : [];

  return (
    <div className="flex h-full flex-col bg-panel outline-none" tabIndex={-1} onKeyDown={onKeyDown}>
      <div className="flex min-h-8 shrink-0 flex-wrap items-center gap-x-1 gap-y-1 border-b border-line bg-panel-2 px-2 py-1">
        <TextButton icon="run" label={paused ? t("Continue") : t("Run")} accent disabled={!(paused || startable)} onClick={run} />
        <TextButton icon="run-cursor" label={t("Step")} disabled={!(paused || startable)} onClick={step} />
        <TextButton icon="rollback" label={t("Stop")} disabled={!live || finished} onClick={() => tellDebug(panel.id, "stop")} />
        {finished && (
          <>
            <span className="mx-1 h-4 w-px bg-line" />
            <TextButton icon="commit" label={t("Commit")} onClick={() => tellDebug(panel.id, "commit")} />
            <TextButton icon="rollback" label={t("Roll Back")} onClick={() => tellDebug(panel.id, "rollback")} />
          </>
        )}
        <span className="ml-2 text-muted" role="status">
          {connected ? t(states[panel.state], { line: panel.line ?? "" }) : t("Not connected.")}
        </span>
      </div>
      {panel.error && (
        <p className="shrink-0 border-b border-line px-2 py-1.5 whitespace-pre-wrap text-danger select-text" role="alert">
          {panel.line !== null && panel.state === "failed" ? `${t("Line {line}", { line: panel.line })}: ` : ""}
          {panel.error}
        </p>
      )}
      {target && target.notes.length > 0 && !live && (
        <p className="shrink-0 border-b border-line px-2 py-1.5 text-muted">
          {t("The debugger emulates the routine, and cannot run all of it. It will stop with an error at:")}{" "}
          {target.notes.map((n) => t("line {line} ({what})", { line: n.line, what: n.what })).join("; ")}
        </p>
      )}
      {target && (
        <div className="flex min-h-0 flex-1">
          <div className="flex min-w-0 flex-1 flex-col">
            <Source panel={panel} />
            <div className="flex h-2/5 max-h-72 min-h-24 shrink-0 flex-col border-t border-line">
              <div className={heading}>
                {t("Rows suspended")}
                <span className="font-normal text-muted tabular-nums">
                  {panel.rowCount > panel.rows.length
                    ? t("{shown} of {n}", { shown: panel.rows.length.toLocaleString(), n: panel.rowCount.toLocaleString() })
                    : panel.rowCount.toLocaleString()}
                </span>
                {panel.notes.length > 0 && (
                  <span className="truncate font-normal text-muted" title={panel.notes.map((n) => `${n.line}: ${n.what}`).join("\n")}>
                    {t("Passed over: {notes}", { notes: panel.notes.map((n) => t("line {line} ({what})", { line: n.line, what: n.what })).join("; ") })}
                  </span>
                )}
              </div>
              <div className="min-h-0 flex-1 overflow-auto">
                {target.outputs.length === 0 ? (
                  <p className="px-2 py-1.5 text-muted">{t("The routine returns nothing.")}</p>
                ) : (
                  <table className="border-collapse text-left select-text">
                    <thead className="sticky top-0 bg-panel">
                      <tr>
                        {target.outputs.map((name) => (
                          <th key={name} className="border-r border-b border-line px-2 py-0.5 font-semibold">
                            {name}
                          </th>
                        ))}
                      </tr>
                    </thead>
                    <tbody>
                      {panel.rows.map((row, i) => (
                        <tr key={i}>
                          {row.map((value, j) => (
                            <td key={j} className={`max-w-96 truncate border-r border-b border-line px-2 py-0.5 ${value === null ? "text-faint" : ""}`}>
                              {value ?? "NULL"}
                            </td>
                          ))}
                        </tr>
                      ))}
                    </tbody>
                  </table>
                )}
              </div>
            </div>
          </div>
          <div className="flex w-80 shrink-0 flex-col overflow-auto border-l border-line">
            {target.inputs.length > 0 && (
              <>
                <div className={heading}>{t("Parameters")}</div>
                <div className="flex flex-col gap-1.5 border-b border-line px-2 py-2">
                  {target.inputs.map((input, i) => (
                    <label key={input.name} className="flex flex-col gap-0.5">
                      <span className="flex justify-between gap-2">
                        <span className="truncate">{input.name}</span>
                        <span className="shrink-0 text-faint">{input.typeName}</span>
                      </span>
                      <input
                        className="h-6 rounded border border-line bg-panel px-1.5 text-fg outline-none focus:border-accent disabled:text-muted"
                        value={panel.inputs[i] ?? ""}
                        placeholder="NULL"
                        disabled={live}
                        spellCheck={false}
                        onChange={(e) => setDebugInput(panel.id, i, e.target.value)}
                      />
                    </label>
                  ))}
                  <p className="text-faint">{t("Left empty, a parameter is NULL: its default is not applied.")}</p>
                </div>
              </>
            )}
            <div className={heading}>{t("Variables")}</div>
            {variables.length === 0 ? (
              <p className="px-2 py-1.5 text-muted">{t("They show when the run stops.")}</p>
            ) : (
              <ul className="select-text">
                {variables.map((v) => (
                  <li key={v.name} className="flex items-baseline gap-2 border-b border-line px-2 py-0.5" title={v.typeName}>
                    <span className="w-6 shrink-0 text-faint">{kinds[v.kind]}</span>
                    <span className="min-w-0 shrink-0 basis-2/5 truncate">{v.name}</span>
                    <span
                      className={`min-w-0 flex-1 truncate font-mono ${v.value === null ? "text-faint" : panel.changed.includes(v.name) ? "text-accent" : ""}`}
                      title={v.value ?? "NULL"}
                    >
                      {v.value ?? "NULL"}
                    </span>
                  </li>
                ))}
              </ul>
            )}
          </div>
        </div>
      )}
    </div>
  );
}
