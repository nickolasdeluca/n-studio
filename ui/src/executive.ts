// The Script Executive: scripts as work in themselves, run all or in part
// on a connection of their own. The backend runs them and tells each
// statement's outcome; here is what each panel shows of it.
import { listen } from "@tauri-apps/api/event";
import { open, save } from "@tauri-apps/plugin-dialog";
import { create } from "zustand";
import {
  api,
  errorText,
  type ExecutiveEnd,
  type ExecutiveOptions,
  type ExecutivePasswordAsk,
  type ExecutiveProgress,
  type ExecutiveStep,
  type FileFormat,
  type Operation,
  type ScriptBlock,
  type SubjectKind,
} from "./api";
import { metadataChanged } from "./changes";
import { baseName, openAdminPanel, panelTitle, setPanelTitle } from "./dock";
import { ms } from "./editors";
import { t } from "./i18n";
import { useStore } from "./store";

export type ExecutiveTab = "script" | "statements" | "options";

export interface ExecutiveMessage {
  level: "info" | "error";
  text: string;
  /** The statement it is about, by index, and its line when it was run */
  index?: number;
  line?: number;
}

export interface ExecutiveRun {
  id: number;
  /** The statements chosen, by index, in order; all of them without it */
  chosen: number[] | null;
  picked: Set<number> | null;
  total: number;
  /** How many of them are through */
  at: number;
  failed: number;
  started: number;
}

export interface Executive {
  id: string;
  /** The database it starts on; none, and the script's CONNECT says */
  registration: string | null;
  /** The character set the text is sent in; none, and the database's */
  charset: string | null;
  options: ExecutiveOptions;
  /** The script's statements, as last outlined */
  blocks: ScriptBlock[];
  /** Statements left out of Run Checked, by index; all are checked at first */
  unchecked: Record<number, true>;
  /** What happened to each statement in the last run, by index */
  steps: Record<number, ExecutiveStep>;
  run: ExecutiveRun | null;
  /** A run whose transaction is still active, for the user to end */
  active: number | null;
  messages: ExecutiveMessage[];
  tab: ExecutiveTab;
  /** A statement to bring into sight in the Statements tab */
  focus: number | null;
  /** The explorer of what the script touches is shown */
  explorer: boolean;
  /** A file run from disk, not loaded in the editor: what is run while
   *  there is one */
  disk: { path: string; size: number; total: number } | null;
  /** The file the text is from, and how it is written */
  file: { path: string; format: FileFormat } | null;
  changed: boolean;
}

interface ExecutiveState {
  panels: Record<string, Executive>;
}

export const useExecutives = create<ExecutiveState>(() => ({ panels: {} }));

export const defaultOptions: ExecutiveOptions = { stopOnError: true, autoDdl: true, rollbackOnStop: true, atEnd: "ask" };

/** Messages kept: a script with everything wrong must not bury the panel */
const MAX_MESSAGES = 2000;

/** The text of each mounted panel's editor, and how to put text in it */
export const texts = new Map<string, { read(): string; write(text: string): void }>();

export function patchExecutive(id: string, p: Partial<Executive> | ((e: Executive) => Partial<Executive>)) {
  useExecutives.setState((s) => {
    const panel = s.panels[id];
    if (!panel) return s;
    return { panels: { ...s.panels, [id]: { ...panel, ...(typeof p === "function" ? p(panel) : p) } } };
  });
}

const get = (id: string) => useExecutives.getState().panels[id];

let count = 0;

/** Opens a Script Executive, on a registration or on none */
export function openExecutive(registration: string | null): string {
  count += 1;
  const id = `adm-exe-${count}`;
  const panel: Executive = {
    id,
    registration,
    charset: null,
    options: defaultOptions,
    blocks: [],
    unchecked: {},
    steps: {},
    run: null,
    active: null,
    messages: [],
    tab: "script",
    explorer: true,
    focus: null,
    disk: null,
    file: null,
    changed: false,
  };
  useExecutives.setState((s) => ({ panels: { ...s.panels, [id]: panel } }));
  openAdminPanel(id, "executive", titleOf(panel, count));
  return id;
}

function titleOf(panel: Executive, n = Number(panel.id.slice("adm-exe-".length))) {
  if (panel.file) return `${baseName(panel.file.path)}${panel.changed ? " •" : ""}`;
  const name = useStore.getState().registrations.find((r) => r.id === panel.registration)?.name;
  return `${t("Script Executive")} ${n}${name ? ` · ${name}` : ""}`;
}

export function retitle(id: string) {
  const panel = get(id);
  if (panel) setPanelTitle(id, titleOf(panel));
}

// ── Running ───────────────────────────────────────────────────────────────

/** What each run does with its progress, by run */
const progress = new Map<number, (event: ExecutiveProgress) => void>();
/** Each run's end, by panel: what closing and quitting wait for */
const ends = new Map<string, Promise<unknown>>();
let subscribed: Promise<unknown> | null = null;

function listening(): Promise<unknown> {
  subscribed ??= Promise.all([
    listen<ExecutiveProgress>("executive-progress", (e) => progress.get(e.payload.run)?.(e.payload)),
    listen<ExecutivePasswordAsk>("executive-password", (e) => askPassword(e.payload)),
  ]).catch(() => {
    // Outside Tauri there are no events: the run's end still tells all
  });
  return subscribed;
}

function askPassword(ask: ExecutivePasswordAsk) {
  const panel = Object.values(useExecutives.getState().panels).find((p) => p.run?.id === ask.run);
  if (!panel) {
    void api.executivePassword(ask.run, null);
    return;
  }
  useStore.getState().openDialog({ type: "scriptPassword", ...ask, title: panelTitle(panel.id) });
}

let runs = 0;
const newRun = () => Date.now() * 1000 + (runs++ % 1000);

function log(id: string, messages: ExecutiveMessage[]) {
  if (messages.length === 0) return;
  patchExecutive(id, (p) => {
    const all = p.messages.concat(messages);
    return { messages: all.length > MAX_MESSAGES ? all.slice(all.length - MAX_MESSAGES) : all };
  });
}

/** The messages a run's steps call for */
function stepMessages(id: string, steps: ExecutiveStep[]): ExecutiveMessage[] {
  const out: ExecutiveMessage[] = [];
  const blocks = get(id)?.blocks ?? [];
  const line = (index: number) => blocks[index]?.index === index ? blocks[index].line : blocks.find((b) => b.index === index)?.line;
  for (const step of steps) {
    if (step.status === "failed") out.push({ level: "error", text: step.message, index: step.index, line: line(step.index) });
    else if (step.status === "skipped" && step.note) out.push({ level: "info", text: step.note, index: step.index, line: line(step.index) });
  }
  return out;
}

function take(id: string, run: number, steps: ExecutiveStep[], at?: number) {
  const panel = get(id);
  if (!panel?.run || panel.run.id !== run) return;
  const failed = steps.filter((s) => s.status === "failed").length;
  patchExecutive(id, (p) => ({
    steps: { ...p.steps, ...Object.fromEntries(steps.map((s) => [s.index, s])) },
    run: p.run && { ...p.run, at: at ?? p.run.at + steps.length, failed: p.run.failed + failed },
  }));
  log(id, stepMessages(id, steps));
}

const n = (v: number) => v.toLocaleString();

/** Runs the panel's script, or the statements of it numbered in `only` */
export async function runExecutive(id: string, only: number[] | null = null): Promise<void> {
  const panel = get(id);
  if (!panel || panel.run) return;
  if (panel.disk) return runDisk(id, only);
  const text = texts.get(id)?.read() ?? "";
  if (panel.active !== null) {
    log(id, [{ level: "error", text: t("The last run left a transaction active: commit or roll back first.") }]);
    return;
  }
  if (!text.trim()) {
    log(id, [{ level: "info", text: t("Nothing to execute.") }]);
    return;
  }
  const run = newRun();
  const done = (async () => {
    let blocks: ScriptBlock[];
    try {
      // The statements as the run will number them
      blocks = await api.executiveOutline(text);
    } catch (e) {
      log(id, [{ level: "error", text: errorText(e) }]);
      return;
    }
    if (only && blocks.length !== get(id)?.blocks.length) {
      // What was checked was another text's statements
      patchExecutive(id, { blocks, unchecked: {} });
      log(id, [{ level: "error", text: t("The script changed since its statements were listed: check them again.") }]);
      return;
    }
    const picked = new Set(only ?? []);
    const chosen = only ? blocks.map((b) => b.index).filter((i) => picked.has(i)) : blocks.map((b) => b.index);
    if (chosen.length === 0) {
      patchExecutive(id, { blocks });
      log(id, [{ level: "info", text: t("No statement is checked.") }]);
      return;
    }
    const run_ = { id: run, chosen: only ? chosen : null, picked: only ? new Set(chosen) : null, total: chosen.length };
    patchExecutive(id, { blocks, steps: {}, messages: [], run: { ...run_, at: 0, failed: 0, started: Date.now() } });
    await listening();
    progress.set(run, (e) => take(id, run, e.steps, e.at));
    try {
      const options = get(id)?.options ?? defaultOptions;
      const end = await api.executiveRun(run, panel.registration, panel.charset, { type: "text", text }, only, options);
      take(id, run, end.steps);
      finish(id, run, end);
    } catch (e) {
      log(id, [{ level: "error", text: errorText(e) }]);
    } finally {
      progress.delete(run);
      patchExecutive(id, { run: null });
    }
  })();
  ends.set(id, done);
  await done;
  if (ends.get(id) === done) ends.delete(id);
}

/** Runs the file the panel has on disk, or the statements of it in `only` */
async function runDisk(id: string, only: number[] | null): Promise<void> {
  const panel = get(id);
  if (!panel?.disk || panel.run) return;
  if (panel.active !== null) {
    log(id, [{ level: "error", text: t("The last run left a transaction active: commit or roll back first.") }]);
    return;
  }
  const run = newRun();
  const listed = panel.blocks.map((b) => b.index);
  // Past the statements listed, all are run unless some are left out
  const chosen = only ? listed.filter((i) => only.includes(i)) : null;
  const done = (async () => {
    const run_ = { id: run, chosen, picked: chosen && new Set(chosen), total: chosen?.length ?? panel.disk!.total };
    patchExecutive(id, { steps: {}, messages: [], run: { ...run_, at: 0, failed: 0, started: Date.now() } });
    if (only && listed.length < panel.disk!.total) {
      log(id, [{ level: "info", text: t("Only the statements listed can be checked: those past them are not run.") }]);
    }
    await listening();
    progress.set(run, (e) => take(id, run, e.steps, e.at));
    try {
      const options = get(id)?.options ?? defaultOptions;
      const end = await api.executiveRun(run, panel.registration, panel.charset, { type: "file", path: panel.disk!.path }, only, options);
      take(id, run, end.steps);
      finish(id, run, end);
    } catch (e) {
      log(id, [{ level: "error", text: errorText(e) }]);
    } finally {
      progress.delete(run);
      patchExecutive(id, { run: null });
    }
  })();
  ends.set(id, done);
  await done;
  if (ends.get(id) === done) ends.delete(id);
}

/** Asks for a script file, lists its statements and runs it from disk,
 *  without loading it into the editor */
export async function runFromFile(id: string): Promise<void> {
  const panel = get(id);
  if (!panel || panel.run) return;
  if (panel.active !== null) {
    log(id, [{ level: "error", text: t("The last run left a transaction active: commit or roll back first.") }]);
    return;
  }
  let path: string | null;
  try {
    const picked = await open({ title: t("Run Script from File"), multiple: false, filters: filters() });
    path = typeof picked === "string" ? picked : null;
  } catch (e) {
    useStore.getState().setNotice(errorText(e));
    return;
  }
  if (!path) return;
  patchExecutive(id, { messages: [{ level: "info", text: t("Reading {file}…", { file: baseName(path) }) }] });
  try {
    const outline = await api.executiveOutlineFile(path);
    patchExecutive(id, {
      disk: { path, size: outline.size, total: outline.total },
      blocks: outline.blocks,
      unchecked: {},
      steps: {},
      messages: [],
      tab: "script",
    });
    if (outline.blocks.length < outline.total) {
      log(id, [
        {
          level: "info",
          text: t("{total} statements; the first {listed} are listed.", {
            total: n(outline.total),
            listed: n(outline.blocks.length),
          }),
        },
      ]);
    }
  } catch (e) {
    log(id, [{ level: "error", text: errorText(e) }]);
    return;
  }
  await runDisk(id, null);
}

/** Back to the script in the editor */
export function closeDisk(id: string) {
  const panel = get(id);
  if (!panel?.disk || panel.run) return;
  patchExecutive(id, { disk: null, blocks: [], steps: {}, unchecked: {} });
  void outlineExecutive(id);
}

function finish(id: string, run: number, end: ExecutiveEnd) {
  const counts = t("{done} executed, {failed} failed, {skipped} skipped of {statements} statement(s) in {time}", {
    done: n(end.done),
    failed: n(end.failed),
    skipped: n(end.skipped),
    statements: n(end.statements),
    time: ms(end.elapsedMs),
  });
  const how = end.cancelled
    ? t("Script stopped")
    : end.stopped
      ? t("Script stopped at an error")
      : end.failed > 0
        ? t("Script finished with errors")
        : t("Script finished");
  const messages: ExecutiveMessage[] = [{ level: end.failed > 0 ? "error" : "info", text: `${how}: ${counts}` }];
  if (end.endError) messages.push({ level: "error", text: t("The commit failed and was rolled back: {error}", { error: end.endError }) });
  else if (end.ended === "commit") messages.push({ level: "info", text: t("Committed.") });
  else if (end.ended === "rollback") messages.push({ level: "info", text: t("Rolled back.") });
  if (end.active) messages.push({ level: "info", text: t("The transaction is still active: commit or roll back.") });
  log(id, messages);
  if (end.active) patchExecutive(id, { active: run });
  const registration = get(id)?.registration;
  if (end.ddl > 0 && registration) metadataChanged(registration);
}

/** Stops the panel's run: before its next statement, and the running one */
export function stopExecutive(id: string) {
  const run = get(id)?.run;
  if (run) void api.executiveCancel(run.id);
}

/** Ends the transaction a run left active. The error, if it failed. */
export async function endExecutive(id: string, how: "commit" | "rollback"): Promise<string | null> {
  const run = get(id)?.active;
  if (run == null) return null;
  try {
    await api.executiveEnd(run, how);
    log(id, [{ level: "info", text: how === "commit" ? t("Committed.") : t("Rolled back.") }]);
    return null;
  } catch (e) {
    const error = errorText(e);
    log(id, [{ level: "error", text: t("The commit failed and was rolled back: {error}", { error }) }]);
    return error;
  } finally {
    // A failed commit is rolled back: nothing is active either way
    patchExecutive(id, { active: null });
  }
}

/** Panels a closing or a quit would stop or end the transaction of */
export function busyExecutives(only?: string): string[] {
  return Object.values(useExecutives.getState().panels)
    .filter((p) => (only === undefined || p.id === only) && (p.run !== null || p.active !== null))
    .map((p) => p.id);
}

/** Stops the panel's run, waits for it, and ends what it left active */
export async function settleExecutive(id: string, how: "commit" | "rollback"): Promise<string | null> {
  stopExecutive(id);
  await ends.get(id);
  return endExecutive(id, how);
}

/** The panel closed: whatever it left is let go */
export function closeExecutive(id: string) {
  const panel = get(id);
  if (!panel) return;
  if (panel.run) void api.executiveCancel(panel.run.id);
  if (panel.active !== null) void api.executiveEnd(panel.active, "rollback").catch(() => {});
  texts.delete(id);
  useExecutives.setState((s) => {
    const panels = { ...s.panels };
    delete panels[id];
    return { panels };
  });
}

/** The statement running now, by index */
export function runningIndex(run: ExecutiveRun | null): number | undefined {
  if (!run || run.at >= run.total) return undefined;
  return run.chosen ? run.chosen[run.at] : run.at;
}

// ── The statements ────────────────────────────────────────────────────────

/** Outlines the panel's text again: after typing, debounced by the panel */
export async function outlineExecutive(id: string): Promise<void> {
  const panel = get(id);
  if (!panel || panel.run || panel.disk) return;
  try {
    const blocks = await api.executiveOutline(texts.get(id)?.read() ?? "");
    const before = get(id);
    if (!before || before.run || before.disk) return;
    // Other statements: what was checked and what ran were about the old ones
    const same = blocks.length === before.blocks.length;
    patchExecutive(id, { blocks, ...(same ? {} : { unchecked: {}, steps: {} }) });
  } catch {
    // Outlining again is for show; the run outlines what it sends
  }
}

/** The statements checked, by index */
export function checkedOf(panel: Executive): number[] {
  return panel.blocks.map((b) => b.index).filter((i) => !panel.unchecked[i]);
}

/** Checks the statements `which` says, unchecking the rest */
export function checkOnly(id: string, which: (index: number) => boolean) {
  patchExecutive(id, (p) => ({ unchecked: Object.fromEntries(p.blocks.filter((b) => !which(b.index)).map((b) => [b.index, true])) }));
}

export function toggleChecked(id: string, index: number) {
  patchExecutive(id, (p) => {
    const unchecked = { ...p.unchecked };
    if (unchecked[index]) delete unchecked[index];
    else unchecked[index] = true;
    return { unchecked };
  });
}

const verbs: Record<Operation, string> = {
  create: "Create",
  alter: "Alter",
  createOrAlter: "Create or alter",
  recreate: "Recreate",
  drop: "Drop",
  comment: "Comment",
  grant: "Grant",
  revoke: "Revoke",
  insert: "Insert into",
  update: "Update",
  updateOrInsert: "Update or insert into",
  delete: "Delete from",
  merge: "Merge into",
  select: "Select",
  execute: "Execute",
  commit: "Commit",
  rollback: "Roll back",
  setGenerator: "Set generator",
  connect: "Connect",
  other: "",
};

const kindNames: Record<SubjectKind, string> = {
  table: "table",
  view: "view",
  procedure: "procedure",
  function: "function",
  package: "package",
  trigger: "trigger",
  domain: "domain",
  generator: "generator",
  exception: "exception",
  index: "index",
  role: "role",
  user: "user",
  udf: "UDF",
  database: "database",
  block: "block",
};

/** What a statement does, in a few words: "Alter procedure P",
 *  "Comment table T.C"; empty when it is not known */
export function operationLabel(block: ScriptBlock): string {
  const { operation, kind, name, member } = block.subject;
  if (operation === "other") return "";
  const verb = t(verbs[operation]);
  // DML names its table without saying "table"
  const dml = ["insert", "update", "updateOrInsert", "delete", "merge", "setGenerator"].includes(operation);
  const what = kind && !dml && !(operation === "execute" && kind === "procedure") ? t(kindNames[kind]) : "";
  const object = name ? (member && kind !== "index" && kind !== "trigger" ? `${name}.${member}` : name) : "";
  return [verb, what, object].filter(Boolean).join(" ");
}

// ── Files ─────────────────────────────────────────────────────────────────

const filters = () => [
  { name: t("SQL scripts"), extensions: ["sql", "ddl", "dml", "txt"] },
  { name: t("All files"), extensions: ["*"] },
];

/** Asks for a script and puts it in the panel's editor */
export async function openExecutiveFile(id: string) {
  try {
    const path = await open({ title: t("Open Script"), multiple: false, filters: filters() });
    if (typeof path !== "string") return;
    const { text, format } = await api.scriptRead(path);
    patchExecutive(id, { disk: null });
    texts.get(id)?.write(text);
    patchExecutive(id, { file: { path, format }, changed: false, steps: {}, messages: [] });
    retitle(id);
  } catch (e) {
    useStore.getState().setNotice(errorText(e));
  }
}

/** Writes the panel's text to its file, asking where when it has none */
export async function saveExecutive(id: string, as = false) {
  const panel = get(id);
  if (!panel) return;
  try {
    let path = as ? null : (panel.file?.path ?? null);
    path ??= await save({ title: t("Save Script"), defaultPath: panel.file?.path ?? "script.sql", filters: filters() });
    if (!path) return;
    const format = panel.file?.format ?? { encoding: "utf8", crlf: false };
    const written = await api.scriptWrite(path, texts.get(id)?.read() ?? "", format);
    patchExecutive(id, { file: { path, format: written }, changed: false });
    retitle(id);
  } catch (e) {
    useStore.getState().setNotice(errorText(e));
  }
}

/** The editor's text changed */
export function executiveEdited(id: string) {
  const panel = get(id);
  if (!panel || panel.changed) return;
  patchExecutive(id, { changed: true });
  if (panel.file) retitle(id);
}
