// The PSQL debugger's tabs. The backend follows the routine, in a thread
// and a transaction of the run's own, and says where it is; this keeps
// what it said, and tells it what next.
import { listen } from "@tauri-apps/api/event";
import { create } from "zustand";
import { api, errorText, type DebugCommand, type DebugNote, type DebugState, type DebugTarget, type DebugVar, type ObjectKind } from "./api";
import { openAdminPanel } from "./dock";
import { t } from "./i18n";
import { useStore } from "./store";

export interface DebugPanel {
  /** Its panel's id too */
  id: string;
  registration: string;
  kind: ObjectKind;
  name: string;
  target?: DebugTarget;
  /** What was typed for each input; nothing typed is NULL */
  inputs: string[];
  /** Lines, from 1 */
  breakpoints: number[];
  run: number;
  /** `idle` before a run and `running` between two stops; the rest are
   *  the backend's */
  state: "loading" | "idle" | "running" | DebugState["state"];
  line: number | null;
  endLine: number | null;
  variables: DebugVar[];
  /** The variables whose value is not what it was at the last stop */
  changed: string[];
  rows: (string | null)[][];
  rowCount: number;
  /** What the run passed over */
  notes: DebugNote[];
  error?: string;
}

export const useDebug = create<{ panels: Record<string, DebugPanel> }>(() => ({ panels: {} }));

const patch = (id: string, p: Partial<DebugPanel>) =>
  useDebug.setState((s) => (s.panels[id] ? { panels: { ...s.panels, [id]: { ...s.panels[id], ...p } } } : s));

/** Whether a run is alive: it can be told something */
export const alive = (panel: DebugPanel) => ["running", "paused", "finished"].includes(panel.state);

let count = 0;
let runs = 0;
/** The panel of each run */
const running = new Map<number, string>();
let subscribed: Promise<unknown> | null = null;

function heard(event: DebugState) {
  const id = running.get(event.run);
  const panel = id ? useDebug.getState().panels[id] : undefined;
  if (!id || !panel) return;
  const before = new Map(panel.variables.map((v) => [v.name, v.value]));
  patch(id, {
    state: event.state,
    line: event.line,
    endLine: event.endLine,
    variables: event.variables,
    changed: event.variables.filter((v) => before.has(v.name) && before.get(v.name) !== v.value).map((v) => v.name),
    rows: [...panel.rows, ...event.rows],
    rowCount: event.rowCount,
    notes: [...panel.notes, ...event.notes],
    error: event.error ?? undefined,
  });
  if (["ended", "stopped", "failed"].includes(event.state)) running.delete(event.run);
}

function listening(): Promise<unknown> {
  subscribed ??= listen<DebugState>("debug-state", (event) => heard(event.payload)).catch(() => {
    // Outside Tauri there are no events, and nothing to debug
  });
  return subscribed;
}

/** Starts a run: to its first statement with `step`, to its first
 *  breakpoint or its end without */
export async function startDebug(id: string, step: boolean) {
  const panel = useDebug.getState().panels[id];
  if (!panel?.target || alive(panel)) return;
  await listening();
  const run = ++runs;
  running.set(run, id);
  patch(id, { run, state: "running", line: null, endLine: null, variables: [], changed: [], rows: [], rowCount: 0, notes: [], error: undefined });
  try {
    const inputs = panel.target.inputs.map((_, i) => panel.inputs[i] || null);
    await api.debugStart(panel.registration, run, panel.kind, panel.name, inputs, panel.breakpoints, step);
  } catch (e) {
    running.delete(run);
    patch(id, { state: "failed", error: errorText(e) });
  }
}

export async function tellDebug(id: string, command: DebugCommand) {
  const panel = useDebug.getState().panels[id];
  if (!panel || !alive(panel)) return;
  if (command === "step" || command === "continue") patch(id, { state: "running" });
  try {
    await api.debugCommand(panel.run, command, panel.breakpoints);
  } catch (e) {
    patch(id, { error: errorText(e) });
  }
}

export function toggleBreakpoint(id: string, line: number) {
  const panel = useDebug.getState().panels[id];
  if (!panel) return;
  const breakpoints = panel.breakpoints.includes(line) ? panel.breakpoints.filter((l) => l !== line) : [...panel.breakpoints, line];
  patch(id, { breakpoints });
  // A run on its way learns of it at its next statement
  if (alive(panel)) void api.debugCommand(panel.run, "", breakpoints).catch(() => {});
}

export const setDebugInput = (id: string, index: number, text: string) => {
  const panel = useDebug.getState().panels[id];
  if (panel) patch(id, { inputs: panel.inputs.map((x, i) => (i === index ? text : x)) });
};

/** A closed tab's run is stopped, and what it did undone */
export function forgetDebug(id: string) {
  const panel = useDebug.getState().panels[id];
  if (panel && alive(panel)) void api.debugCommand(panel.run, panel.state === "finished" ? "rollback" : "stop", null).catch(() => {});
  useDebug.setState((s) => {
    const panels = { ...s.panels };
    delete panels[id];
    return { panels };
  });
}

/** Opens a procedure or a function in the debugger */
export function openDebugger(registration: string, kind: ObjectKind, name: string) {
  if (!useStore.getState().servers[registration]) return;
  const id = `adm-dbg-${++count}`;
  useDebug.setState((s) => ({
    panels: {
      ...s.panels,
      [id]: { id, registration, kind, name, inputs: [], breakpoints: [], run: 0, state: "loading", line: null, endLine: null, variables: [], changed: [], rows: [], rowCount: 0, notes: [] },
    },
  }));
  openAdminPanel(id, "debug", `${t("Debug")} · ${name}`);
  api.debugOpen(registration, kind, name).then(
    (target) => patch(id, { target, inputs: target.inputs.map(() => ""), state: "idle" }),
    (e) => patch(id, { state: "failed", error: errorText(e) }),
  );
}
