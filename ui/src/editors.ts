// The SQL editors' view state: each tab's transaction, its last statement's
// outcome and its message log. The text lives in the tab's CodeMirror view,
// the rows in the backend.
import { create } from "zustand";
import {
  api,
  errorText,
  isAppError,
  type ColumnInfo,
  type Executed,
  type ExportOptions,
  type Isolation,
  type ResultView,
  type StatementRange,
  type TxEnd,
  type TxOptions,
} from "./api";
import { t } from "./i18n";
import { askParams, forgetParams, paramFields } from "./params";

/** Rows fetched with the execute, and per page as the grid scrolls */
export const PAGE = 200;

export type BottomTab = "results" | "messages" | "columns" | "plan" | "performance";

export interface Message {
  at: number;
  level: "info" | "error";
  text: string;
  /** The statement it is about, in the editor's text */
  range?: { from: number; to: number };
}

export interface Outcome {
  executed: Executed;
  /** The select's rows as fetched so far; updated as more come */
  result: ResultView | null;
  sql: string;
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
  messages: Message[];
  bottom: BottomTab;
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
  open(registration: string): string;
  close(id: string): Promise<void>;
  setTxOptions(id: string, options: Partial<TxOptions>): void;
  setBottom(id: string, bottom: BottomTab): void;
  run(id: string, mode: RunMode, input: RunInput, onError?: (range: { from: number; to: number }) => void): Promise<void>;
  end(id: string, how: TxEnd): Promise<void>;
  fetchMore(id: string, max: number): Promise<void>;
  /** Sorts the last select's rows by a column: ascending, descending, then
   *  back to fetch order. Fetches the rest first. */
  sort(id: string, column: number): Promise<void>;
  /** Writes the last select's rows to a file, fetching the rest first */
  exportRows(id: string, path: string, options: ExportOptions): Promise<void>;
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

export const useEditors = create<State>()((set, get) => {
  const patch = (id: string, p: Partial<EditorTab> | ((tab: EditorTab) => Partial<EditorTab>)) =>
    set((s) => {
      const tab = s.tabs[id];
      if (!tab) return s;
      return { tabs: { ...s.tabs, [id]: { ...tab, ...(typeof p === "function" ? p(tab) : p) } } };
    });
  const log = (id: string, level: Message["level"], text: string, range?: Message["range"]) =>
    patch(id, (tab) => ({ messages: [...tab.messages, { at: Date.now(), level, text, range }] }));

  /** Drops the backend's rows of the tab's last select */
  const closeResult = (tab: EditorTab) => {
    const result = tab.outcome?.result;
    if (result) void api.resultClose(result.id).catch(() => {});
  };

  return {
    tabs: {},

    open(registration) {
      opened += 1;
      const id = `sql-${Date.now().toString(36)}-${opened}`;
      set((s) => ({
        tabs: {
          ...s.tabs,
          [id]: {
            id,
            registration,
            tx: null,
            txOptions: { isolation: "read_committed", readOnly: false, lockWait: { mode: "wait" } },
            running: false,
            fetching: false,
            outcome: null,
            messages: [],
            bottom: "results",
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
      set((s) => {
        const tabs = { ...s.tabs };
        delete tabs[id];
        return { tabs };
      });
      // A closing editor's work is rolled back, never committed behind the
      // user's back
      if (tab.tx !== null) await api.editorEnd(tab.registration, tab.tx, "rollback").catch(() => {});
    },

    setTxOptions(id, options) {
      patch(id, (tab) => ({ txOptions: { ...tab.txOptions, ...options } }));
    },

    setBottom(id, bottom) {
      patch(id, { bottom });
    },

    async run(id, mode, input, onError) {
      const start = get().tabs[id];
      if (!start || start.running) return;
      patch(id, { running: true });
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

    async end(id, how) {
      const tab = get().tabs[id];
      if (!tab || tab.tx === null || tab.running) return;
      patch(id, { running: true });
      try {
        await api.editorEnd(tab.registration, tab.tx, how);
        log(id, "info", endedText(how));
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

    async cancel(id) {
      const tab = get().tabs[id];
      if (!tab || !(tab.running || tab.fetching)) return;
      try {
        if (await api.editorCancel(tab.registration)) log(id, "info", t("Cancelling…"));
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
      const executed = await api.editorExecute(
        tab.registration,
        tab.tx!,
        bound.sql,
        params,
        { plan: detailed, performance: detailed },
        PAGE,
      );
      log(id, "info", executedText(executed), range);
      if (executed.ended === "commit" || executed.ended === "rollback") patch(id, { tx: null });
      // Only a statement with something to show replaces the last outcome
      if (executed.result || executed.columns.length > 0 || detailed) {
        closeResult(get().tabs[id]!);
        patch(id, (tab) => ({
          outcome: { executed, result: executed.result, sql },
          bottom: tab.bottom === "messages" && executed.result ? "results" : tab.bottom,
        }));
      }
      return "ok";
    } catch (e) {
      const where = summary(sql);
      log(id, "error", `${errorText(e)}\n${t("Statement: {sql}", { sql: where })}`, range);
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
