// The SQL editors' view state: each tab's transaction, its last statement's
// outcome and its message log. The text lives in the tab's CodeMirror view,
// the rows in the backend.
import { create } from "zustand";
import {
  api,
  type ColumnSummary,
  errorText,
  isAppError,
  type ColumnInfo,
  type Executed,
  type RowEditing,
  type ExportOptions,
  type FileFormat,
  type Isolation,
  type ResultView,
  type ScriptProblem,
  type StatementRange,
  type TxEnd,
  type TxOptions,
} from "./api";
import { metadataChanged, onMetadataChanged } from "./changes";
import { record } from "./history";
import { t } from "./i18n";
import { newRun, runScript } from "./script";
import { askParams, forgetParams, paramFields } from "./params";
import { editCount, noEdits, type RowEdits } from "./rowEdits";
import { selectEditing, tableOf, type SelectEditing } from "./selectEdit";
import { blanked, sources } from "./sqlScope";

/** Rows fetched with the execute, and per page as the grid scrolls */
export const PAGE = 200;

export type BottomTab = "results" | "messages" | "history" | "columns" | "plan" | "performance";

export interface Message {
  at: number;
  level: "info" | "error";
  text: string;
  /** The statement it is about, in the text of the page it ran on */
  range?: { from: number; to: number; page?: string };
}

/** At most this many pages to an editor */
export const MAX_PAGES = 10;

/** One of an editor's texts. Its pages share the editor's transaction and
 *  its bottom: what runs is the page in front's text. */
export interface Page {
  id: string;
  /** Its number in the editor, for its name while it has no other */
  n: number;
  /** The name it was given, if one was */
  title: string | null;
  /** The script it was opened from or saved to */
  file: string | null;
  format: FileFormat | null;
  /** Its text is no longer what `file` holds */
  changed: boolean;
}

/** What a page is opened with */
export interface PageSeed {
  text: string;
  file?: EditorFile;
  title?: string | null;
}

export interface Outcome {
  executed: Executed;
  /** The select's rows as fetched so far; updated as more come */
  result: ResultView | null;
  sql: string;
  /** What went to the server, and the values its parameters were given:
   *  what reads the rows again once they were changed */
  sent: { sql: string; params: (string | null)[] };
  /** How the rows can be changed in the grid, when they are one table's
   *  and came with its key; known a moment after the rows are */
  edit: SelectEditing | null;
  /** What the run before this one took in all, when it was of the same
   *  statement: prepare, execute and what it had fetched */
  previousMs?: number;
  /** Read again after its rows were changed: the grid stays where it was */
  again?: boolean;
}

export interface EditorTab {
  id: string;
  /** The registration it runs on */
  registration: string;
  tx: number | null;
  txOptions: TxOptions;
  running: boolean;
  /** Fetching more rows of the last select */
  fetching: boolean;
  outcome: Outcome | null;
  /** What was done to the last select's rows in the grid and not applied */
  edits: RowEdits;
  messages: Message[];
  bottom: BottomTab;
  /** The bottom shows its tabs only, and the text has the room */
  minimized: boolean;
  /** The script it is running, and how far that is */
  script: { run: number; at: number; statements: number; failed: number } | null;
  scriptOptions: { stopOnError: boolean; autoDdl: boolean };
  /** Behind an object viewer's Data tab rather than an editor of the
   *  user's: read only, and ended without asking */
  viewer: boolean;
  /** The script it was opened from or saved to */
  /** In the order they are shown, never none */
  pages: Page[];
  /** The page in front */
  page: string;
  /** The page the last run was of: what its messages' places are in */
  ranOn: string | null;
}

/** The page in front, or the one with the id */
export const pageOf = (tab: EditorTab, id: string = tab.page): Page | undefined => tab.pages.find((p) => p.id === id);

/** Where an editor's text is, when it is a file's */
export interface EditorFile {
  file: string;
  format: FileFormat;
  changed: boolean;
}

export type RunMode = "current" | "all" | "to_cursor";

/** What a run needs from the editor's text */
export interface RunInput {
  text: string;
  /** The selection, or the cursor when it is empty */
  from: number;
  to: number;
}

interface State {
  tabs: Record<string, EditorTab>;
  /** `viewer`: for an object viewer's Data tab. `pages` are its texts,
   *  one empty page when there are none; `front` the one in front. */
  open(registration: string, viewer?: boolean, pages?: PageSeed[], front?: number): string;
  /** A new page after the one in front, which comes to the front; null when
   *  the editor has as many as it can */
  addPage(id: string, seed?: PageSeed): string | null;
  /** Its last page is not closed */
  closePage(id: string, page: string): void;
  showPage(id: string, page: string): void;
  /** Gives a page a name; blank, it goes back to its file's or its number */
  renamePage(id: string, page: string, title: string): void;
  /** The page in front was typed in */
  edited(id: string): void;
  /** The page's text is now what `file` holds */
  stored(id: string, page: string, file: string, format: FileFormat): void;
  /** Says something in the tab's messages */
  note(id: string, text: string): void;
  close(id: string): Promise<void>;
  setTxOptions(id: string, options: Partial<TxOptions>): void;
  /** Shows a bottom tab, the bottom restored if it was minimized */
  setBottom(id: string, bottom: BottomTab): void;
  setMinimized(id: string, minimized: boolean): void;
  run(id: string, mode: RunMode, input: RunInput, onError?: (range: { from: number; to: number }) => void): Promise<void>;
  /** Runs the text (or its selection) as a script: every statement, in the
   *  backend, with what fails logged and the rest carrying on or not */
  runScript(id: string, input: RunInput, onError?: (range: { from: number; to: number }) => void): Promise<void>;
  setScriptOptions(id: string, options: Partial<EditorTab["scriptOptions"]>): void;
  end(id: string, how: TxEnd): Promise<void>;
  fetchMore(id: string, max: number): Promise<void>;
  /** Sorts the last select's rows by a column: ascending, descending, then
   *  back to fetch order. Fetches the rest first. */
  sort(id: string, column: number): Promise<void>;
  /** What columns of the last select add up to, fetching every row first;
   *  null when it could not be worked out */
  summarize(id: string, columns: number[]): Promise<ColumnSummary[] | null>;
  /** Writes the last select's rows to a file, fetching the rest first */
  exportRows(id: string, path: string, options: ExportOptions): Promise<void>;
  setEdits(id: string, edits: RowEdits): void;
  /** Reads the last select's rows again, as they are after a change */
  again(id: string): Promise<void>;
  /** Stops the statement or fetch the tab is running */
  cancel(id: string): Promise<void>;
  /** The registration disconnected: its transactions and results are gone */
  sessionClosed(registration: string): void;
}

export const isolations: { value: Isolation; label: string }[] = [
  { value: "read_committed", label: "Read Committed" },
  { value: "read_committed_no_rec_version", label: "Read Committed (No Rec Version)" },
  { value: "snapshot", label: "Snapshot" },
  { value: "snapshot_table_stability", label: "Snapshot Table Stability" },
];

let opened = 0;
/** The text a new page opens with, by page, until its panel takes it */
export const drafts = new Map<string, string>();
/** How to read each page's text, which lives in its panel */
export const readers = new Map<string, () => string>();

/** A page's text as it is now */
export const textOf = (page: string): string => readers.get(page)?.() ?? drafts.get(page) ?? "";

/** Makes a page of `tab` from a seed, its text held until it is shown */
function newPage(tab: string, n: number, seed: PageSeed): Page {
  const id = `${tab}:${n}`;
  if (seed.text) drafts.set(id, seed.text);
  return {
    id,
    n,
    title: seed.title ?? null,
    file: seed.file?.file ?? null,
    format: seed.file?.format ?? null,
    changed: seed.file?.changed ?? false,
  };
}

/** The text of every page of an editor goes with it */
function forgetPages(tab: EditorTab) {
  for (const page of tab.pages) {
    drafts.delete(page.id);
    readers.delete(page.id);
  }
}

/** How each table's rows can be changed, by registration and table: asked
 *  once, and again after the catalog changed */
const editings = new Map<string, Promise<RowEditing | null>>();
onMetadataChanged((registration) => {
  for (const key of editings.keys()) if (key.startsWith(`${registration}:`)) editings.delete(key);
});

/** What the outcome before took, if it was of this same statement */
function tookBefore(before: Outcome | null, sql: string): number | undefined {
  if (!before || before.sql.trim() !== sql.trim()) return undefined;
  return before.executed.prepareMs + before.executed.executeMs + (before.result?.fetchMs ?? 0);
}

/** Of one script's statements, how many problems the log takes */
const MAX_PROBLEMS = 500;

export const useEditors = create<State>()((set, get) => {
  const patch = (id: string, p: Partial<EditorTab> | ((tab: EditorTab) => Partial<EditorTab>)) =>
    set((s) => {
      const tab = s.tabs[id];
      if (!tab) return s;
      return { tabs: { ...s.tabs, [id]: { ...tab, ...(typeof p === "function" ? p(tab) : p) } } };
    });
  /** A place in the text is in the page the run was of */
  const log = (id: string, level: Message["level"], text: string, range?: Message["range"]) =>
    patch(id, (tab) => ({
      messages: [...tab.messages, { at: Date.now(), level, text, range: range && { ...range, page: tab.ranOn ?? tab.page } }],
    }));

  /** Drops the backend's rows of the tab's last select */
  const closeResult = (tab: EditorTab) => {
    const result = tab.outcome?.result;
    if (result) void api.resultClose(result.id).catch(() => {});
  };

  return {
    tabs: {},

    open(registration, viewer = false, seeds = [], front = 0) {
      opened += 1;
      const id = `${viewer ? "data" : "sql"}-${Date.now().toString(36)}-${opened}`;
      const pages = (seeds.length > 0 ? seeds.slice(0, MAX_PAGES) : [{ text: "" }]).map((seed, i) => newPage(id, i + 1, seed));
      set((s) => ({
        tabs: {
          ...s.tabs,
          [id]: {
            id,
            registration,
            tx: null,
            txOptions: { isolation: "read_committed", readOnly: viewer, lockWait: { mode: "wait" } },
            running: false,
            fetching: false,
            outcome: null,
            edits: noEdits,
            messages: [],
            bottom: "results",
            minimized: false,
            script: null,
            scriptOptions: { stopOnError: false, autoDdl: true },
            viewer,
            pages,
            page: (pages[front] ?? pages[0]).id,
            ranOn: null,
          },
        },
      }));
      return id;
    },

    async close(id) {
      const tab = get().tabs[id];
      if (!tab) return;
      closeResult(tab);
      forgetParams(id);
      forgetPages(tab);
      set((s) => {
        const tabs = { ...s.tabs };
        delete tabs[id];
        return { tabs };
      });
      // A closing editor's work is rolled back, never committed behind the
      // user's back
      if (tab.tx !== null) await api.editorEnd(tab.registration, tab.tx, "rollback").catch(() => {});
    },

    addPage(id, seed = { text: "" }) {
      const tab = get().tabs[id];
      if (!tab || tab.pages.length >= MAX_PAGES) return null;
      const page = newPage(id, Math.max(...tab.pages.map((p) => p.n)) + 1, seed);
      const at = tab.pages.findIndex((p) => p.id === tab.page) + 1;
      patch(id, { pages: [...tab.pages.slice(0, at), page, ...tab.pages.slice(at)], page: page.id });
      return page.id;
    },

    closePage(id, page) {
      const tab = get().tabs[id];
      if (!tab || tab.pages.length <= 1) return;
      const at = tab.pages.findIndex((p) => p.id === page);
      if (at < 0) return;
      const pages = tab.pages.filter((p) => p.id !== page);
      drafts.delete(page);
      readers.delete(page);
      // The one after it comes to the front, or the one before the last
      patch(id, { pages, page: tab.page === page ? pages[Math.min(at, pages.length - 1)].id : tab.page });
    },

    renamePage(id, page, title) {
      const name = title.trim() || null;
      patch(id, (tab) => ({ pages: tab.pages.map((p) => (p.id === page && p.title !== name ? { ...p, title: name } : p)) }));
    },

    showPage(id, page) {
      const tab = get().tabs[id];
      if (tab && tab.page !== page && pageOf(tab, page)) patch(id, { page });
    },

    edited(id) {
      const tab = get().tabs[id];
      const page = tab && pageOf(tab);
      if (page?.file && !page.changed) patch(id, { pages: tab.pages.map((p) => (p === page ? { ...p, changed: true } : p)) });
    },

    stored(id, page, file, format) {
      patch(id, (tab) => ({ pages: tab.pages.map((p) => (p.id === page ? { ...p, file, format, changed: false } : p)) }));
    },

    note(id, text) {
      log(id, "info", text);
    },

    setTxOptions(id, options) {
      patch(id, (tab) => ({ txOptions: { ...tab.txOptions, ...options } }));
    },

    setBottom(id, bottom) {
      patch(id, { bottom, minimized: false });
    },

    setMinimized(id, minimized) {
      patch(id, { minimized });
    },

    async run(id, mode, input, onError) {
      const start = get().tabs[id];
      if (!start || start.running) return;
      patch(id, { running: true, ranOn: start.page });
      try {
        const statements = await pick(mode, input);
        if (statements.length === 0) {
          log(id, "info", t("Nothing to execute."));
          return;
        }
        // A script runs without plans and counters: one round trip less per
        // statement, and only the last one's would show
        const single = statements.filter((s) => s.kind.type === "sql").length === 1;
        for (const statement of statements) {
          const range = { from: statement.from, to: statement.to };
          if (statement.kind.type === "set_term") continue;
          if (statement.kind.type === "isql") {
            log(id, "info", t("Skipped an isql command: {sql}", { sql: summary(statement.sql) }), range);
            continue;
          }
          const ran = await runOne(id, statement.sql, single, range);
          if (ran === "failed") onError?.(range);
          if (ran !== "ok") break;
        }
      } catch (e) {
        log(id, "error", errorText(e));
      } finally {
        patch(id, { running: false });
      }
    },

    setScriptOptions(id, options) {
      patch(id, (tab) => ({ scriptOptions: { ...tab.scriptOptions, ...options } }));
    },

    async runScript(id, input, onError) {
      const start = get().tabs[id];
      if (!start || start.running) return;
      const selected = input.from !== input.to;
      const offset = selected ? input.from : 0;
      const text = selected ? input.text.slice(input.from, input.to) : input.text;
      if (!text.trim()) {
        log(id, "info", t("Nothing to execute."));
        return;
      }
      const run = newRun();
      let shown = 0;
      let first: { from: number; to: number } | null = null;
      const report = (problems: ScriptProblem[]) => {
        for (const p of problems) {
          const range = { from: p.from + offset, to: p.to + offset };
          const sql = summary(text.slice(p.from, p.to));
          if (p.error !== null) first ??= range;
          // A script with everything wrong must not bury the app in messages
          shown += 1;
          if (shown > MAX_PROBLEMS) continue;
          if (p.error === null) log(id, "info", t("Skipped an isql command: {sql}", { sql }), range);
          else log(id, "error", `${p.error}\n${t("Statement {n}: {sql}", { n: n(p.index + 1), sql })}`, range);
        }
      };
      patch(id, (tab) => ({ running: true, ranOn: tab.page, script: { run, at: 0, statements: 0, failed: 0 } }));
      try {
        const options = { ...start.scriptOptions, atomic: false };
        const end = await runScript(run, start.registration, start.tx, start.txOptions, text, options, (event) => {
          report(event.problems);
          patch(id, { script: { run, at: event.at, statements: event.statements, failed: event.failed } });
        });
        report(end.problems);
        if (shown > MAX_PROBLEMS) log(id, "info", t("{n} more message(s) not shown.", { n: n(shown - MAX_PROBLEMS) }));
        const counts = t("{done} executed, {failed} failed, {skipped} skipped of {statements} statement(s) in {time}", {
          done: n(end.done),
          failed: n(end.failed),
          skipped: n(end.skipped),
          statements: n(end.statements),
          time: ms(end.elapsedMs),
        });
        const how = end.cancelled ? t("Script stopped") : end.failed > 0 ? t("Script finished with errors") : t("Script finished");
        // A script is one line of history, not one for each of its statements
        record(start.registration, text, end.elapsedMs, end.failed > 0 ? `${how}: ${counts}` : null);
        log(id, end.failed > 0 ? "error" : "info", `${how}: ${counts}`);
        if (end.tx !== null) log(id, "info", t("The transaction is still active: commit or roll back."));
        patch(id, (tab) => ({
          tx: end.tx,
          bottom: "messages",
          // The rows a transaction was showing end with it
          outcome:
            tab.outcome?.result && tab.tx !== end.tx
              ? { ...tab.outcome, result: { ...tab.outcome.result, eof: true, endedEarly: !tab.outcome.result.eof } }
              : tab.outcome,
        }));
        if (end.ddl > 0) metadataChanged(start.registration);
        if (first) onError?.(first);
      } catch (e) {
        log(id, "error", errorText(e));
        patch(id, { bottom: "messages" });
        if (isAppError(e) && e.kind === "not_connected") patch(id, { tx: null });
      } finally {
        patch(id, { running: false, script: null });
      }
    },

    async end(id, how) {
      const tab = get().tabs[id];
      if (!tab || tab.tx === null || tab.running) return;
      patch(id, { running: true });
      try {
        await api.editorEnd(tab.registration, tab.tx, how);
        log(id, "info", endedText(how));
        if (how === "rollback" || how === "rollback_retaining") {
          // What a rolled back transaction read is no longer there to change:
          // its rows go, with what was edited in them and not applied
          const now = get().tabs[id];
          if (now?.outcome?.result) {
            closeResult(now);
            patch(id, { outcome: null, edits: noEdits });
          }
        }
        if (how === "commit" || how === "rollback") {
          patch(id, (tab) => ({
            tx: null,
            outcome: tab.outcome?.result
              ? { ...tab.outcome, result: { ...tab.outcome.result, eof: true, endedEarly: !tab.outcome.result.eof } }
              : tab.outcome,
          }));
        }
      } catch (e) {
        log(id, "error", errorText(e));
        if (isAppError(e) && e.kind === "not_connected") patch(id, { tx: null });
      } finally {
        patch(id, { running: false });
      }
    },

    async fetchMore(id, max) {
      const result = get().tabs[id]?.outcome?.result;
      if (!result || result.eof || get().tabs[id]?.fetching) return;
      patch(id, { fetching: true });
      try {
        const view = await api.resultFetch(result.id, max);
        patch(id, (tab) =>
          tab.outcome?.result?.id === view.id ? { outcome: { ...tab.outcome, result: view } } : {},
        );
      } catch (e) {
        log(id, "error", errorText(e));
        // The rows fetched before the failure (or the cancel) stay, and the
        // cursor is gone: ask for nothing more, to learn how many there are
        const view = await api.resultFetch(result.id, 0).catch(() => ({ ...result, eof: true }));
        patch(id, (tab) =>
          tab.outcome?.result?.id === result.id ? { outcome: { ...tab.outcome, result: view } } : {},
        );
      } finally {
        patch(id, { fetching: false });
      }
    },

    async sort(id, column) {
      const tab = get().tabs[id];
      const result = tab?.outcome?.result;
      if (!result || tab.running || tab.fetching) return;
      const now = result.sort?.column === column ? result.sort : null;
      const next = !now ? { column, descending: false } : !now.descending ? { column, descending: true } : null;
      await whileFetching(id, result.id, () => api.resultSort(result.id, next));
    },

    async summarize(id, columns) {
      const tab = get().tabs[id];
      const result = tab?.outcome?.result;
      if (!result || tab.running || tab.fetching) return null;
      let summaries: ColumnSummary[] | null = null;
      await whileFetching(id, result.id, async () => {
        const [view, got] = await api.resultSummary(result.id, columns);
        summaries = got;
        return view;
      });
      return summaries;
    },

    async exportRows(id, path, options) {
      const tab = get().tabs[id];
      const result = tab?.outcome?.result;
      if (!result || tab.running || tab.fetching) return;
      await whileFetching(id, result.id, async () => {
        const summary = await api.resultExport(result.id, path, options);
        log(id, "info", t("{n} row(s) exported to {path}", { n: n(summary.rows), path }));
        if (summary.blobsUnread) {
          log(id, "info", t("The transaction had ended: blob columns were exported as (MEMO) and (BLOB)."));
        }
        return api.resultFetch(result.id, 0);
      });
    },

    setEdits(id, edits) {
      patch(id, { edits });
    },

    async again(id) {
      const tab = get().tabs[id];
      const outcome = tab?.outcome;
      if (!tab || !outcome?.result || tab.running || tab.fetching) return;
      patch(id, { running: true });
      try {
        let tx = tab.tx;
        if (tx === null) {
          tx = await api.editorBegin(tab.registration, tab.txOptions);
          patch(id, { tx });
        }
        const { plan, performance } = outcome.executed;
        const options = { plan: plan !== null, performance: performance !== null };
        // As many rows as were on the screen, so the grid can stay where it is
        const rows = Math.min(Math.max(PAGE, outcome.result.rows), 5000);
        const executed = await api.editorExecute(tab.registration, tx, outcome.sent.sql, outcome.sent.params, options, rows);
        closeResult(tab);
        patch(id, { outcome: { ...outcome, executed, result: executed.result, again: true }, edits: noEdits });
      } catch (e) {
        log(id, "error", errorText(e));
        patch(id, { bottom: "messages" });
      } finally {
        patch(id, { running: false });
      }
    },

    async cancel(id) {
      const tab = get().tabs[id];
      if (!tab || !(tab.running || tab.fetching)) return;
      try {
        const sent = tab.script ? api.scriptCancel(tab.registration, tab.script.run) : api.editorCancel(tab.registration);
        if (await sent) log(id, "info", t("Cancelling…"));
      } catch (e) {
        log(id, "error", errorText(e));
      }
    },

    sessionClosed(registration) {
      for (const tab of Object.values(get().tabs)) {
        if (tab.registration !== registration) continue;
        patch(tab.id, (tab) => ({
          tx: null,
          outcome: tab.outcome?.result
            ? { ...tab.outcome, result: { ...tab.outcome.result, eof: true, endedEarly: !tab.outcome.result.eof } }
            : tab.outcome,
        }));
        if (tab.tx !== null) log(tab.id, "info", t("Disconnected: the transaction was rolled back."));
      }
    },
  };

  /** Runs `work`, which may fetch the rest of a result, with the tab showing
   *  it fetches; the result's view is what `work` returns. On a failure (or
   *  a cancel) the rows fetched so far stay. */
  async function whileFetching(id: string, resultId: number, work: () => Promise<ResultView>) {
    const update = (view: ResultView) =>
      patch(id, (tab) => (tab.outcome?.result?.id === view.id ? { outcome: { ...tab.outcome, result: view } } : {}));
    patch(id, { fetching: true });
    try {
      update(await work());
    } catch (e) {
      log(id, "error", errorText(e));
      patch(id, { bottom: "messages" });
      await api.resultFetch(resultId, 0).then(update, () => {});
    } finally {
      patch(id, { fetching: false });
    }
  }

  /** Finds whether a select's rows can be changed in the grid: they are one
   *  table's, which has a key, and the select brought it */
  async function findEditing(id: string, executed: Executed) {
    const tab = get().tabs[id];
    const sql = tab?.outcome?.executed === executed ? tab.outcome.sent.sql : null;
    if (!tab || sql === null) return;
    // A union's columns are said to be its first select's, whose rows only
    // some of them are
    if (/\bunion\b/i.test(blanked(sql))) return;
    const table = tableOf(executed.columns, sources(sql, 0).map((s) => s.relation));
    if (!table) return;
    const key = `${tab.registration}:${table}`;
    let asked = editings.get(key);
    if (!asked) {
      asked = api.rowEditing(tab.registration, table).catch(() => null);
      editings.set(key, asked);
    }
    const editing = await asked;
    const edit = editing && selectEditing(executed.columns, table, editing);
    if (!edit) return;
    // Still the result it was asked for
    patch(id, (now) => (now.outcome?.executed === executed ? { outcome: { ...now.outcome, edit } } : {}));
  }

  /** Runs one statement, starting a transaction first if there is none,
   *  and asking its parameters' values if it has any */
  async function runOne(
    id: string,
    sql: string,
    detailed: boolean,
    range: { from: number; to: number },
  ): Promise<"ok" | "failed" | "cancelled"> {
    let tab = get().tabs[id];
    if (!tab) return "failed";
    // When the statement went to the server, if it got that far
    let sent: number | null = null;
    try {
      if (tab.tx === null) {
        const tx = await api.editorBegin(tab.registration, tab.txOptions);
        patch(id, { tx });
        tab = get().tabs[id]!;
      }
      // No ? and no : is no parameter, without asking the backend
      let bound = { sql, names: [] as (string | null)[] };
      if (/[?:]/.test(sql)) bound = await api.statementParameters(sql);
      let params: (string | null)[] = [];
      if (bound.names.length > 0) {
        const types = await api.editorDescribe(tab.registration, tab.tx!, bound.sql);
        const values = await askParams(id, paramFields(bound.names, types));
        if (!values) {
          log(id, "info", t("Not executed: no parameter values given."), range);
          return "cancelled";
        }
        params = values;
      }
      sent = performance.now();
      const executed = await api.editorExecute(
        tab.registration,
        tab.tx!,
        bound.sql,
        params,
        { plan: detailed, performance: detailed },
        PAGE,
      );
      log(id, "info", executedText(executed), range);
      // As typed, with its parameters' names: their values are not kept
      if (!tab.viewer) record(tab.registration, sql, executed.prepareMs + executed.executeMs, null);
      if (executed.ended === "commit" || executed.ended === "rollback") patch(id, { tx: null });
      // Only a statement with something to show replaces the last outcome
      if (executed.result || executed.columns.length > 0 || detailed) {
        const before = get().tabs[id]!;
        closeResult(before);
        const dropped = editCount(before.edits);
        if (dropped > 0) log(id, "info", t("{n} changed row(s) of the last result were not applied, and went with it.", { n: dropped }));
        patch(id, (tab) => ({
          outcome: { executed, result: executed.result, sql, sent: { sql: bound.sql, params }, edit: null, previousMs: tookBefore(before.outcome, sql) },
          edits: noEdits,
          bottom: tab.bottom === "messages" && executed.result ? "results" : tab.bottom,
        }));
        if (executed.result && !tab.viewer) void findEditing(id, executed);
      }
      return "ok";
    } catch (e) {
      const where = summary(sql);
      log(id, "error", `${errorText(e)}\n${t("Statement: {sql}", { sql: where })}`, range);
      if (sent !== null && !tab.viewer) record(tab.registration, sql, performance.now() - sent, errorText(e));
      patch(id, { bottom: "messages" });
      if (isAppError(e) && e.kind === "not_connected") patch(id, { tx: null });
      return "failed";
    }
  }
});

interface Picked {
  sql: string;
  from: number;
  to: number;
  kind: StatementRange["kind"];
}

/** The statements a run covers: those in the selection, or by the cursor */
async function pick(mode: RunMode, input: RunInput): Promise<Picked[]> {
  const selected = input.from !== input.to;
  const offset = selected ? input.from : 0;
  const text = selected ? input.text.slice(input.from, input.to) : input.text;
  const all = (await api.sqlStatements(text)).map((s) => ({
    sql: text.slice(s.from, s.to),
    from: s.from + offset,
    to: s.to + offset,
    kind: s.kind,
  }));
  if (selected || mode === "all") return all;

  // The statement the cursor is in or, between statements, the one before it
  const cursor = input.from;
  let at = all.findLastIndex((s) => s.from <= cursor);
  if (at < 0) at = 0;
  if (mode === "to_cursor") return all.slice(0, at + 1);
  // SET TERM before it still applies; only the statement itself runs
  return all.slice(at, at + 1).filter((s) => s.kind.type !== "set_term");
}

function summary(sql: string): string {
  const flat = sql.replace(/\s+/g, " ").trim();
  return flat.length > 120 ? `${flat.slice(0, 117)}…` : flat;
}

const n = (v: number) => v.toLocaleString();

export function ms(v: number): string {
  return v < 10 ? `${v.toFixed(1)} ms` : `${n(Math.round(v))} ms`;
}

function executedText(e: Executed): string {
  const time = t("prepare {prepare}, execute {execute}", { prepare: ms(e.prepareMs), execute: ms(e.executeMs) });
  switch (e.statementType) {
    case "select":
    case "select_for_update": {
      const r = e.result;
      const rows = r ? (r.eof ? t("{n} row(s) fetched", { n: n(r.rows) }) : t("first {n} row(s) fetched", { n: n(r.rows) })) : "";
      return `${t("Query executed")}: ${rows} (${time})`;
    }
    case "insert":
      return `${t("{n} row(s) inserted", { n: n(e.records.inserted) })} (${time})`;
    case "update":
      return `${t("{n} row(s) updated", { n: n(e.records.updated) })} (${time})`;
    case "delete":
      return `${t("{n} row(s) deleted", { n: n(e.records.deleted) })} (${time})`;
    case "exec_procedure":
      return `${t("Procedure executed")} (${time})`;
    case "commit":
    case "rollback":
      return endedText(e.ended ?? "commit");
    case "ddl":
      return `${t("Statement executed; commit to make the change visible to other transactions")} (${time})`;
    default:
      return `${t("Statement executed")} (${time})`;
  }
}

function endedText(how: TxEnd): string {
  return {
    commit: t("Transaction committed."),
    commit_retaining: t("Transaction committed (retaining)."),
    rollback: t("Transaction rolled back."),
    rollback_retaining: t("Transaction rolled back (retaining)."),
  }[how];
}

export function columnsOf(tab: EditorTab | undefined): ColumnInfo[] {
  return tab?.outcome?.executed.columns ?? [];
}
