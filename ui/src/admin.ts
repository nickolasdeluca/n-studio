// Looking after a server and a database: the activity monitor, the server's
// users, and the services (backup, restore, validation, sweep, statistics,
// the server's log). The monitor and the users are grids the backend draws,
// with their actions; a service runs on the server and writes lines.
import { listen } from "@tauri-apps/api/event";
import { create } from "zustand";
import { api, errorText, type Action, type AdminView, type GridSection, type ServiceOutput } from "./api";
import type { MenuItem } from "./components/ContextMenu";
import { openBuilder } from "./builder";
import { openDiagram } from "./diagram";
import { openAdminPanel } from "./dock";
import { t } from "./i18n";
import { useStore, type Dialog } from "./store";

// ── The monitor and the users ───────────────────────────────────────────

export interface AdminPanel {
  /** Its panel's id too: one of each view per registration */
  id: string;
  registration: string;
  view: AdminView;
  sections?: GridSection[];
  loading: boolean;
  error?: string;
  /** The monitor's idle statements too */
  idle: boolean;
  /** When what it shows was read */
  at?: Date;
}

interface AdminState {
  panels: Record<string, AdminPanel>;
  open(registration: string, view: AdminView): string;
  close(id: string): void;
  load(id: string): Promise<void>;
  setIdle(id: string, idle: boolean): void;
}

export const adminId = (registration: string, view: AdminView) => `adm-${view}-${registration}`;

export const useAdmin = create<AdminState>((set, get) => {
  const patch = (id: string, p: Partial<AdminPanel>) =>
    set((s) => (s.panels[id] ? { panels: { ...s.panels, [id]: { ...s.panels[id], ...p } } } : s));
  return {
    panels: {},
    open(registration, view) {
      const id = adminId(registration, view);
      if (!get().panels[id]) {
        set((s) => ({ panels: { ...s.panels, [id]: { id, registration, view, loading: false, idle: false } } }));
        void get().load(id);
      }
      return id;
    },
    close(id) {
      set((s) => {
        const panels = { ...s.panels };
        delete panels[id];
        return { panels };
      });
    },
    async load(id) {
      const panel = get().panels[id];
      if (!panel || panel.loading) return;
      patch(id, { loading: true });
      try {
        const sections = await api.adminView(panel.registration, panel.view, panel.idle);
        patch(id, { sections, loading: false, error: undefined, at: new Date() });
      } catch (e) {
        patch(id, { loading: false, error: errorText(e) });
      }
    },
    setIdle(id, idle) {
      patch(id, { idle });
      void get().load(id);
    },
  };
});

const plain = (label: string) => t(label).replace(/…$/, "");

/** Starts an action of a panel's grid: its form when it asks for values,
 *  its script to confirm otherwise. `key` is the row it acts on. */
export async function startAdminAction(panel: AdminPanel, action: Action, key: string | null) {
  const s = useStore.getState();
  try {
    if (!action.form) {
      await confirmAdminAction(panel, action, key, {});
      return;
    }
    const form = await api.adminForm(panel.registration, action.id, key);
    s.openDialog({
      type: "form",
      title: `${plain(action.label)}: ${form.title}`,
      form,
      submit: (values, back) => confirmAdminAction(panel, action, key, values, back),
    });
  } catch (e) {
    s.setNotice(errorText(e));
  }
}

async function confirmAdminAction(
  panel: AdminPanel,
  action: Action,
  key: string | null,
  values: Record<string, string>,
  back?: Dialog,
) {
  const { script, title } = await api.adminScript(panel.registration, action.id, key, values);
  useStore.getState().openDialog({
    type: "compile",
    registration: panel.registration,
    title: `${plain(action.label)}: ${title}`,
    script,
    danger: action.danger,
    back,
    // Ending an attachment or a statement compiles nothing
    verb: panel.view === "monitor" ? t("Run") : undefined,
    done: () => void useAdmin.getState().load(panel.id),
  });
}

// ── Services ────────────────────────────────────────────────────────────

export interface ServiceRun {
  /** Its panel's id too */
  id: string;
  run: number;
  title: string;
  state: "running" | "finished" | "cancelled" | "failed";
  error?: string;
  /** How many lines it wrote so far: the lines themselves are in `linesOf` */
  count: number;
  started: Date;
  ended?: Date;
}

/** What each service wrote, by panel: kept out of the store, since a backup
 *  writes a line per table and index and the store would copy them all at
 *  every one */
const output = new Map<string, string[]>();
export const linesOf = (id: string): string[] => output.get(id) ?? [];

export const useServices = create<{ runs: Record<string, ServiceRun> }>(() => ({ runs: {} }));

const patchRun = (id: string, p: Partial<ServiceRun>) =>
  useServices.setState((s) => (s.runs[id] ? { runs: { ...s.runs, [id]: { ...s.runs[id], ...p } } } : s));

function append(id: string, lines: string[]) {
  const all = output.get(id);
  if (!all || lines.length === 0) return;
  for (const line of lines) all.push(line);
  patchRun(id, { count: all.length });
}

let runs = 0;
const listeners = new Map<number, (event: ServiceOutput) => void>();
let subscribed: Promise<unknown> | null = null;

function listening(): Promise<unknown> {
  subscribed ??= listen<ServiceOutput>("service-output", (event) => listeners.get(event.payload.run)?.(event.payload)).catch(
    () => {
      // Outside Tauri there are no events: the service's end still tells all
    },
  );
  return subscribed;
}

/** Runs a service on a registration's server, in a tab that shows what it
 *  writes. Fails before it starts when the values do not make a task. */
async function runService(registration: string, tool: string, title: string, values: Record<string, string>) {
  await listening();
  const run = ++runs;
  const id = `adm-svc-${run}`;
  output.set(id, []);
  useServices.setState((s) => ({
    runs: { ...s.runs, [id]: { id, run, title, state: "running", count: 0, started: new Date() } },
  }));
  listeners.set(run, (event) => append(id, event.lines));
  openAdminPanel(id, "service", title);
  // The dialog that asked is done with; the tab carries on from here
  void api
    .serviceRun(registration, run, tool, values)
    .then(
      (end) => {
        append(id, end.lines);
        patchRun(id, { state: end.end, ended: new Date() });
      },
      (e) => patchRun(id, { state: "failed", error: errorText(e), ended: new Date() }),
    )
    .finally(() => listeners.delete(run));
}

export function cancelService(id: string) {
  const run = useServices.getState().runs[id];
  if (run?.state === "running") void api.serviceCancel(run.run);
}

/** Its tab closed: stop listening, and forget what it wrote */
export function forgetService(id: string) {
  cancelService(id);
  output.delete(id);
  useServices.setState((s) => {
    const runs = { ...s.runs };
    delete runs[id];
    return { runs };
  });
}

interface Tool {
  id: string;
  label: string;
  /** Asked of the backend, which knows the server's version */
  form: boolean;
  /** What to know before starting one that asks nothing else */
  warning?: string;
}

const tools: Tool[] = [
  { id: "backup", label: "Backup…", form: true },
  { id: "restore", label: "Restore…", form: true },
  { id: "statistics", label: "Statistics…", form: true },
  {
    id: "validate",
    label: "Validate…",
    form: false,
    warning:
      "Validation reads the whole database looking for corruption, and changes nothing. On Firebird 3 and later the database can stay in use; before it, the server wants it with no one else connected.",
  },
  {
    id: "sweep",
    label: "Sweep…",
    form: false,
    warning:
      "A sweep reads the whole database to collect the garbage old transactions left. On a large database it takes long and loads the server.",
  },
  { id: "server_log", label: "Server Log", form: false },
];

async function startTool(registration: string, tool: Tool) {
  const s = useStore.getState();
  const name = s.registrations.find((r) => r.id === registration)?.name ?? "";
  const title = `${plain(tool.label)}${name ? ` · ${name}` : ""}`;
  const start = async (values: Record<string, string>) => {
    await runService(registration, tool.id, title, values);
    useStore.getState().openDialog(null);
  };
  try {
    if (!tool.form && !tool.warning) {
      await start({});
      return;
    }
    const form = tool.form ? await api.adminForm(registration, tool.id, null) : { title: name, fields: [] };
    s.openDialog({
      type: "form",
      title: `${plain(tool.label)}: ${form.title}`,
      form,
      note: tool.warning ? t(tool.warning) : t("It runs on the server; what it writes opens in a tab."),
      verb: t("Start"),
      submit: (values) => start(values),
    });
  } catch (e) {
    s.setNotice(errorText(e));
  }
}

/** What can be done to a connected registration's server and database */
export function adminActions(id: string): (MenuItem | "separator")[] {
  const connected = !!useStore.getState().servers[id];
  return [
    { label: t("Activity Monitor"), icon: "search", disabled: !connected, onSelect: () => openAdminView(id, "monitor") },
    { label: t("Users"), icon: "role", disabled: !connected, onSelect: () => openAdminView(id, "users") },
    { label: t("Diagram"), icon: "table", disabled: !connected, onSelect: () => openDiagram(id) },
    { label: t("Query Builder"), icon: "sql", disabled: !connected, onSelect: () => void openBuilder(id) },
    "separator",
    ...tools.map((tool) => ({
      label: t(tool.label),
      icon: tool.id === "backup" || tool.id === "restore" ? "database" : undefined,
      disabled: !connected,
      onSelect: () => void startTool(id, tool),
    })),
  ];
}

export function openAdminView(registration: string, view: AdminView) {
  const s = useStore.getState();
  if (!s.servers[registration]) return;
  const name = s.registrations.find((r) => r.id === registration)?.name ?? "";
  const id = useAdmin.getState().open(registration, view);
  const title = view === "monitor" ? t("Activity Monitor") : t("Users");
  openAdminPanel(id, "admin", `${title}${name ? ` · ${name}` : ""}`);
}
