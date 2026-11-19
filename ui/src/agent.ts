// The agent: the user's own Claude Code, run by the backend in a terminal
// and drawn here with xterm.js. What it asks of the window (a query to run
// where the user sees it, SQL to open) is answered here too, and so is the
// user's sharing of rows with it.
import { listen } from "@tauri-apps/api/event";
import { FitAddon } from "@xterm/addon-fit";
import { Terminal } from "@xterm/xterm";
import { create } from "zustand";
import { api, errorText, SHARE_CHARS, SHARE_ROWS, type AgentRequest, type Cell, type SharedColumn } from "./api";
import { activatePanel, editorTitle, openEditor } from "./dock";
import { useEditors } from "./editors";
import { t } from "./i18n";
import { unsavedViewers } from "./pending";
import { editCount } from "./rowEdits";
import { useStore } from "./store";

const FOLDER = "n-studio.agent.folder";

export type AgentStatus = "stopped" | "starting" | "running" | "ended";

interface AgentState {
  status: AgentStatus;
  /** The start running, which its events are told by */
  run: number | null;
  /** How the last one ended: 127 is the shell not finding `claude` */
  exitCode: number | null;
  error: string | null;
  /** The project folder it starts in, kept across runs; none, and it works
   *  with the databases only, in a folder of the app's own */
  folder: string | null;
  setFolder(folder: string | null): void;
  start(): Promise<void>;
  stop(): Promise<void>;
}

function savedFolder(): string | null {
  try {
    return localStorage.getItem(FOLDER);
  } catch {
    return null;
  }
}

export const useAgent = create<AgentState>()((set, get) => ({
  status: "stopped",
  run: null,
  exitCode: null,
  error: null,
  folder: savedFolder(),

  setFolder(folder) {
    try {
      if (folder) localStorage.setItem(FOLDER, folder);
      else localStorage.removeItem(FOLDER);
    } catch {
      // Chosen again next time
    }
    set({ folder });
  },

  async start() {
    const { folder, status } = get();
    if (status === "starting") return;
    listening();
    const term = terminal();
    term.reset();
    fit.fit();
    set({ status: "starting", error: null, exitCode: null, run: null });
    try {
      const run = await api.agentStart(folder, term.cols, term.rows);
      set({ status: "running", run });
      term.focus();
    } catch (e) {
      set({ status: "stopped", error: errorText(e) });
    }
  },

  async stop() {
    if (get().status === "stopped") return;
    await api.agentStop().catch(() => {});
    set({ status: "ended", run: null });
  },
}));

// The terminal outlives its panel's mounting: dockview can take a panel out
// of the page and put it back, and what was printed must still be there.
let term: Terminal | null = null;
const fit = new FitAddon();

function colors() {
  const css = getComputedStyle(document.documentElement);
  const v = (name: string) => css.getPropertyValue(name).trim();
  return {
    background: v("--panel"),
    foreground: v("--text"),
    cursor: v("--accent"),
    selectionBackground: v("--selected"),
  };
}

export function terminal(): Terminal {
  if (term) return term;
  term = new Terminal({
    fontFamily: 'ui-monospace, "SF Mono", Menlo, Consolas, "Liberation Mono", monospace',
    fontSize: 12.5,
    cursorBlink: true,
    scrollback: 10_000,
    allowProposedApi: false,
    theme: colors(),
  });
  term.loadAddon(fit);
  term.onData((data) => {
    const { run } = useAgent.getState();
    if (run !== null) void api.agentInput(run, data).catch(() => {});
  });
  term.onResize(({ cols, rows }) => {
    const { run } = useAgent.getState();
    if (run !== null) void api.agentResize(run, cols, rows).catch(() => {});
  });
  // The theme follows the system's, as the rest of the window does
  window.matchMedia("(prefers-color-scheme: dark)").addEventListener("change", () => {
    if (term) term.options.theme = colors();
  });
  return term;
}

/** Shows the terminal in `host`, the panel's element */
export function attachTerminal(host: HTMLElement) {
  const t = terminal();
  if (t.element) host.appendChild(t.element);
  else t.open(host);
  fitTerminal();
}

export function fitTerminal() {
  if (!term?.element?.isConnected) return;
  try {
    fit.fit();
  } catch {
    // Not laid out yet: the next resize fits it
  }
}

let heard: Promise<unknown> | null = null;

/** Listens to the backend's agent events, once */
export function listening() {
  heard ??= Promise.all([
    listen<{ run: number; data: string }>("agent-output", (e) => {
      if (e.payload.run === useAgent.getState().run) terminal().write(e.payload.data);
    }),
    listen<{ run: number; code: number | null }>("agent-exit", (e) => {
      if (e.payload.run !== useAgent.getState().run) return;
      useAgent.setState({ status: "ended", run: null, exitCode: e.payload.code });
      terminal().write(`\r\n\x1b[2m${t("[Claude Code ended]")}\x1b[0m\r\n`);
    }),
    listen<AgentRequest>("agent-request", (e) => void answer(e.payload)),
  ]).catch(() => {
    heard = null;
  });
}

async function answer(request: AgentRequest) {
  let reply: unknown;
  try {
    switch (request.kind) {
      case "run":
        reply = await runQuery(request.registration, request.sql);
        break;
      case "open":
        reply = open(request);
        break;
      case "connect":
        reply = await connect(request.registration);
        break;
      case "disconnect":
        reply = await disconnect(request.registration);
        break;
    }
  } catch (e) {
    reply = { ok: false, error: errorText(e) };
  }
  await api.agentReply(request.request, reply).catch(() => {});
}

function registrationName(id: string): string {
  return useStore.getState().registrations.find((r) => r.id === id)?.name ?? "";
}

function open(request: Extract<AgentRequest, { kind: "open" }>) {
  const title = `Claude · ${request.title ?? registrationName(request.registration)}`;
  const tab = openEditor(request.registration, request.sql, { title });
  return tab ? { ok: true } : { ok: false, error: t("The database is not connected in N Studio.") };
}

/** The agent's editor on each registration: its queries are pages of it */
const editors = new Map<string, string>();

// An agent's editor the user sets to read and write, even for a moment, is
// the user's from then on: its transaction may be one that can write
useEditors.subscribe((state) => {
  for (const [registration, id] of editors) {
    if (!state.tabs[id]?.txOptions.readOnly) editors.delete(registration);
  }
});

/** An editor for the agent's query on `registration`, read only: the one it
 *  had, while it is still read only and not busy, or a new one */
function agentEditor(registration: string, sql: string): string | null {
  const editorsState = useEditors.getState();
  const known = editors.get(registration);
  const tab = known ? editorsState.tabs[known] : undefined;
  if (tab && tab.txOptions.readOnly && !tab.running && !tab.fetching && !tab.script) {
    if (tab.pages.length >= 10) editorsState.closePage(tab.id, tab.pages[0].id);
    editorsState.addPage(tab.id, { text: sql });
    activatePanel(tab.id);
    return tab.id;
  }
  const id = openEditor(registration, sql, { title: `Claude · ${registrationName(registration)}` });
  if (!id) return null;
  editorsState.setTxOptions(id, { readOnly: true });
  editors.set(registration, id);
  return id;
}

/** Runs the agent's select where the user sees it, and says how it went
 *  without a row of it: the backend passes on only what it knows to */
async function runQuery(registration: string, sql: string) {
  if (!useStore.getState().servers[registration]) return { ok: false, error: t("The database is not connected in N Studio.") };
  const id = agentEditor(registration, sql);
  if (!id) return { ok: false, error: t("No editor could be opened.") };
  const before = useEditors.getState().tabs[id];
  // Never in a transaction that can write
  if (!before.txOptions.readOnly) return { ok: false, error: t("The agent's editor is not read only.") };
  const seen = before.messages.length;
  const outcomeBefore = before.outcome;
  await useEditors.getState().run(id, "all", { text: sql, from: 0, to: 0 });
  const tab = useEditors.getState().tabs[id];
  if (!tab) return { ok: false, error: t("The editor was closed.") };
  const failed = tab.messages.slice(seen).find((m) => m.level === "error");
  if (failed) return { ok: false, error: failed.text };
  const outcome = tab.outcome;
  if (!outcome || outcome === outcomeBefore) return { ok: false, error: t("The query did not run.") };
  const { executed, result } = outcome;
  return {
    ok: true,
    columns: executed.columns.map((c) => ({ name: c.name, type: c.typeName, nullable: c.nullable })),
    rows: result?.rows ?? 0,
    allFetched: result?.eof ?? true,
    prepareMs: executed.prepareMs,
    executeMs: executed.executeMs,
    fetchMs: result?.fetchMs ?? 0,
    plan: executed.plan,
    explainedPlan: executed.explainedPlan,
    note: result && !result.eof ? t("Only the first rows were fetched; the user can fetch the rest.") : undefined,
  };
}

/** What a grid can share: its columns and the rows fetched, from `from` */
export interface Shareable {
  registration: string;
  label: string;
  columns: SharedColumn[];
  rows(from: number, count: number): Promise<Cell[][]>;
  /** Rows fetched so far */
  fetched: number;
}

/** How many of a share's rows fit the backend's limits */
export function fitShare(rows: Cell[][]): Cell[][] {
  let chars = 0;
  const kept: Cell[][] = [];
  for (const row of rows.slice(0, SHARE_ROWS)) {
    const size = row.reduce((n, v) => n + (v === null ? 4 : new TextEncoder().encode(v).length), 0);
    if (chars + size > SHARE_CHARS) break;
    chars += size;
    kept.push(row);
  }
  return kept;
}

/** Shares rows with the running agent and types where they are into its
 *  prompt, for the user to go on with: the number of rows shared */
export async function share(registration: string, label: string, columns: SharedColumn[], rows: Cell[][]): Promise<number> {
  const { run } = useAgent.getState();
  if (run === null) throw new Error(t("Claude Code is not running."));
  const kept = fitShare(rows);
  const id = await api.agentShare(run, label, registrationName(registration), columns, kept);
  activatePanel("agent");
  const hint =
    kept.length === 1
      ? t("[shared {id}: 1 row of \"{label}\"] ", { id, label })
      : t("[shared {id}: {rows} rows of \"{label}\"] ", { id, rows: kept.length, label });
  await api.agentInput(run, hint);
  terminal().focus();
  return kept.length;
}

/** What an editor's rows are of, for a share's label: the statement that
 *  read them, cut short */
export function editorLabel(tab: string): string {
  const sql = useEditors.getState().tabs[tab]?.outcome?.sql.replace(/\s+/g, " ").trim() ?? "";
  if (!sql) return t("rows of a query");
  return sql.length > 120 ? `${sql.slice(0, 119)}…` : sql;
}

type Reply = { ok: true } | { ok: false; error: string };

/** Connects a registration as the explorer does: with its saved password,
 *  else the user is asked for it, and the answer is the user's */
async function connect(id: string): Promise<Reply> {
  const start = useStore.getState();
  if (start.servers[id]) return { ok: true };
  if (!start.registrations.some((r) => r.id === id)) return { ok: false, error: t("That database is not registered.") };
  const noticeBefore = start.notice;
  await start.connect(id);
  const outcome = (s: ReturnType<typeof useStore.getState>): Reply | null => {
    if (s.servers[id]) return { ok: true };
    // The password dialog is open, or what was typed in it is being tried
    if ((s.dialog?.type === "password" && s.dialog.id === id) || s.connecting[id]) return null;
    if (s.notice && s.notice !== noticeBefore) return { ok: false, error: s.notice };
    return { ok: false, error: t("The user did not connect it.") };
  };
  return new Promise((resolve) => {
    const now = outcome(useStore.getState());
    if (now) return resolve(now);
    const stop = useStore.subscribe((s) => {
      const done = outcome(s);
      if (!done) return;
      stop();
      resolve(done);
    });
  });
}

/** What the user has open on a registration that a disconnection would
 *  end: none, and it can go. The agent's own editors only read, and have
 *  nothing to lose but a statement running. */
export function openWork(id: string): string[] {
  const mine = new Set(editors.values());
  const tabs = Object.values(useEditors.getState().tabs).filter((tab) => tab.registration === id);
  const names = (list: typeof tabs) => list.map((tab) => editorTitle(tab.id)).join(", ");
  const work: string[] = [];
  const open = tabs.filter((tab) => tab.tx !== null && !tab.viewer && !mine.has(tab.id));
  if (open.length > 0) work.push(t("an open transaction in {editors}", { editors: names(open) }));
  const busy = tabs.filter((tab) => tab.running || tab.fetching || tab.script);
  if (busy.length > 0) work.push(t("a statement running in {editors}", { editors: names(busy) }));
  const edited = tabs.filter((tab) => editCount(tab.edits) > 0);
  if (edited.length > 0) work.push(t("grid changes not applied in {editors}", { editors: names(edited) }));
  const viewers = unsavedViewers((registration) => registration === id);
  if (viewers.length > 0) work.push(t("changes not compiled in {n} object viewer(s)", { n: viewers.length }));
  return work;
}

/** Disconnects a registration, unless the user has work open on it */
async function disconnect(id: string): Promise<Reply> {
  if (!useStore.getState().servers[id]) return { ok: true };
  const work = openWork(id);
  if (work.length > 0) {
    return {
      ok: false,
      error: t("{name} was not disconnected: the user has {work}. The user must end that first.", {
        name: registrationName(id),
        work: work.join("; "),
      }),
    };
  }
  await useStore.getState().disconnect(id, true);
  return useStore.getState().servers[id] ? { ok: false, error: t("It is still connected.") } : { ok: true };
}
