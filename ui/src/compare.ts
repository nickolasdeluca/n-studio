// Two tables' rows compared: the source's against the target's. Nothing is
// changed by comparing; what would make the target the source is a script
// for the target's SQL editor.
import { listen } from "@tauri-apps/api/event";
import { create } from "zustand";
import { api, errorText, type CompareProgress, type Comparison, type Form } from "./api";
import { openAdminPanel } from "./dock";
import { t } from "./i18n";
import { useStore } from "./store";

export interface ComparePanel {
  /** Its panel's id too */
  id: string;
  run: number;
  source: string;
  sourceTable: string;
  target: string;
  targetTable: string;
  state: "running" | "done" | "failed";
  progress?: CompareProgress;
  result?: Comparison;
  error?: string;
}

export const useComparisons = create<{ panels: Record<string, ComparePanel> }>(() => ({ panels: {} }));

const patch = (id: string, p: Partial<ComparePanel>) =>
  useComparisons.setState((s) => (s.panels[id] ? { panels: { ...s.panels, [id]: { ...s.panels[id], ...p } } } : s));

let runs = 0;
const listeners = new Map<number, (event: CompareProgress) => void>();
let subscribed: Promise<unknown> | null = null;

function listening(): Promise<unknown> {
  subscribed ??= listen<CompareProgress>("compare-progress", (event) => listeners.get(event.payload.run)?.(event.payload)).catch(
    () => {
      // Outside Tauri there are no events: the comparison's end still tells all
    },
  );
  return subscribed;
}

/** Runs a panel's comparison, again if it ran before */
export async function runComparison(id: string) {
  const panel = useComparisons.getState().panels[id];
  if (!panel) return;
  await listening();
  const run = ++runs;
  patch(id, { run, state: "running", progress: undefined, error: undefined });
  listeners.set(run, (progress) => patch(id, { progress }));
  try {
    const result = await api.dataCompare(run, panel.source, panel.sourceTable, panel.target, panel.targetTable);
    patch(id, { state: "done", result });
  } catch (e) {
    patch(id, { state: "failed", error: errorText(e) });
  } finally {
    listeners.delete(run);
  }
}

export function cancelComparison(id: string) {
  const panel = useComparisons.getState().panels[id];
  if (panel?.state === "running") void api.dataCompareCancel(panel.run);
}

export function forgetComparison(id: string) {
  cancelComparison(id);
  useComparisons.setState((s) => {
    const panels = { ...s.panels };
    delete panels[id];
    return { panels };
  });
}

/** Asks what to compare a table with, then compares in a tab of its own */
export function startComparison(registration: string, table: string) {
  const s = useStore.getState();
  const connected = s.registrations.filter((r) => s.servers[r.id]);
  const own = connected.find((r) => r.id === registration);
  if (!own) return;
  const form: Form = {
    title: table,
    fields: [
      {
        id: "target",
        label: "Target database",
        kind: "select",
        value: own.name,
        options: connected.map((r) => r.name),
        required: true,
        hint: "Among the connected ones. Its rows are what the script would change; this table is the source.",
      },
      { id: "table", label: "Target table", kind: "combo", value: table, options: [], required: true, hint: null },
    ],
  };
  s.openDialog({
    type: "form",
    title: `${t("Compare Data")}: ${form.title}`,
    form,
    note: t("Nothing is changed: the differences open in a tab."),
    verb: t("Compare"),
    submit: async (values) => {
      const target = connected.find((r) => r.name === values.target);
      const targetTable = values.table.trim();
      if (!target || !useStore.getState().servers[target.id]) throw new Error(t("That database is not connected."));
      if (target.id === registration && targetTable === table) throw new Error(t("That is the same table on both sides."));
      const id = `adm-cmp-${++runs}`;
      useComparisons.setState((state) => ({
        panels: {
          ...state.panels,
          [id]: { id, run: 0, source: registration, sourceTable: table, target: target.id, targetTable, state: "running" },
        },
      }));
      openAdminPanel(id, "compare", `${t("Compare")} · ${table}`);
      useStore.getState().openDialog(null);
      void runComparison(id);
    },
  });
}
