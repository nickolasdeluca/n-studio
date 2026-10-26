// The SQL editor: one tab per editor, each with its own transaction.
// The text on top, what the last statement did below.
import { autocompletion, closeBrackets, closeBracketsKeymap, completionKeymap } from "@codemirror/autocomplete";
import { defaultKeymap, history, historyKeymap, indentWithTab } from "@codemirror/commands";
import { bracketMatching, indentOnInput } from "@codemirror/language";
import { highlightSelectionMatches, searchKeymap } from "@codemirror/search";
import { EditorState, Prec, StateEffect, StateField } from "@codemirror/state";
import {
  Decoration,
  drawSelection,
  EditorView,
  highlightActiveLine,
  highlightActiveLineGutter,
  keymap,
  lineNumbers,
  type DecorationSet,
} from "@codemirror/view";
import type { IDockviewPanelProps } from "dockview-react";
import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import type { TxEnd } from "../api";
import { isolations, ms, useEditors, type BottomTab, type EditorTab, type RunMode } from "../editors";
import { editorLook, firebirdSql } from "../firebird";
import { t } from "../i18n";
import { Icon } from "../icons";
import { useStore } from "../store";
import { ResultGrid, type GridCursor } from "./ResultGrid";

/** Editor states by tab, so a panel that remounts keeps its text and undo */
const saved = new Map<string, EditorState>();
/** What the run keys call, by tab: the mounted panel's callbacks. Kept out
 *  of the saved state, which outlives a mount. */
const runners = new Map<string, { run(mode: RunMode): boolean; end(how: TxEnd): boolean }>();

/** A closed editor's text goes with it */
export function forgetEditor(id: string) {
  saved.delete(id);
  runners.delete(id);
}

function runKeys(id: string) {
  const run = (mode: RunMode) => () => runners.get(id)?.run(mode) ?? false;
  const end = (how: TxEnd) => () => runners.get(id)?.end(how) ?? false;
  return Prec.highest(
    keymap.of([
      { key: "F9", run: run("current") },
      { key: "Mod-Enter", run: run("current") },
      { key: "Shift-F9", run: run("all") },
      { key: "Mod-Shift-Enter", run: run("all") },
      { key: "Alt-F9", run: run("to_cursor") },
      { key: "Mod-Alt-Enter", run: run("to_cursor") },
      { key: "Mod-Alt-c", run: end("commit") },
      { key: "Mod-Alt-r", run: end("rollback") },
    ]),
  );
}

const markError = StateEffect.define<{ from: number; to: number } | null>();
const errorField = StateField.define<DecorationSet>({
  create: () => Decoration.none,
  update(marks, tr) {
    for (const e of tr.effects) {
      if (e.is(markError)) {
        return e.value && e.value.to > e.value.from
          ? Decoration.set([Decoration.mark({ class: "cm-error-statement" }).range(e.value.from, e.value.to)])
          : Decoration.none;
      }
    }
    // Editing clears it: the mark is about text that is changing
    return tr.docChanged ? Decoration.none : marks;
  },
  provide: (f) => EditorView.decorations.from(f),
});

export function SqlEditor({ params }: IDockviewPanelProps<{ tab: string }>) {
  const id = params.tab;
  const tab = useEditors((s) => s.tabs[id]);
  const host = useRef<HTMLDivElement>(null);
  const view = useRef<EditorView | null>(null);
  const [pos, setPos] = useState({ line: 1, col: 1 });
  const [split, setSplit] = useState(0.5);
  const [cursor, setCursor] = useState<GridCursor>({ row: 0, col: 0 });
  const body = useRef<HTMLDivElement>(null);

  const select = useCallback((range: { from: number; to: number }, error = false) => {
    const v = view.current;
    if (!v) return;
    const len = v.state.doc.length;
    const r = { from: Math.min(range.from, len), to: Math.min(range.to, len) };
    v.dispatch({
      selection: { anchor: r.from, head: r.to },
      effects: [markError.of(error ? r : null), EditorView.scrollIntoView(r.from, { y: "center" })],
    });
    v.focus();
  }, []);

  const run = useCallback(
    (mode: RunMode) => {
      const v = view.current;
      if (!v) return false;
      const sel = v.state.selection.main;
      v.dispatch({ effects: markError.of(null) });
      void useEditors
        .getState()
        .run(id, mode, { text: v.state.doc.toString(), from: sel.from, to: sel.to }, (r) => select(r, true));
      return true;
    },
    [id, select],
  );
  const end = useCallback(
    (how: TxEnd) => {
      void useEditors.getState().end(id, how);
      return true;
    },
    [id],
  );

  useEffect(() => {
    if (!host.current) return;
    runners.set(id, { run, end });
    const state =
      saved.get(id) ??
      EditorState.create({
        doc: "",
        extensions: [
          runKeys(id),
          lineNumbers(),
          highlightActiveLineGutter(),
          highlightActiveLine(),
          history(),
          drawSelection(),
          indentOnInput(),
          bracketMatching(),
          closeBrackets(),
          autocompletion(),
          highlightSelectionMatches(),
          errorField,
          firebirdSql,
          editorLook,
          EditorState.tabSize.of(2),
          keymap.of([...closeBracketsKeymap, ...defaultKeymap, ...searchKeymap, ...historyKeymap, ...completionKeymap, indentWithTab]),
        ],
      });
    const v = new EditorView({
      state,
      parent: host.current,
      dispatchTransactions(trs, v) {
        v.update(trs);
        const head = v.state.selection.main.head;
        const line = v.state.doc.lineAt(head);
        setPos({ line: line.number, col: head - line.from + 1 });
      },
    });
    view.current = v;
    v.focus();
    return () => {
      saved.set(id, v.state);
      v.destroy();
      view.current = null;
    };
  }, [id, run, end]);

  // A new result starts at its first row
  const resultId = tab?.outcome?.result?.id;
  useEffect(() => {
    setCursor({ row: 0, col: 0 });
  }, [resultId]);

  if (!tab) {
    return <div className="grid h-full place-items-center bg-bg text-muted">{t("This editor is closed.")}</div>;
  }

  const startSplit = (e: React.MouseEvent) => {
    e.preventDefault();
    const rect = body.current?.getBoundingClientRect();
    if (!rect) return;
    const onMove = (m: MouseEvent) => setSplit(Math.max(0.15, Math.min(0.85, (m.clientY - rect.top) / rect.height)));
    const onUp = () => {
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
    };
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
  };

  return (
    <div className="flex h-full flex-col bg-panel">
      <EditorToolbar tab={tab} run={run} end={end} />
      <div ref={body} className="flex min-h-0 flex-1 flex-col">
        <div ref={host} className="min-h-0 overflow-hidden" style={{ height: `${split * 100}%` }} />
        <div
          className="h-[5px] shrink-0 cursor-row-resize border-y border-line bg-panel-2 hover:bg-accent/40"
          onMouseDown={startSplit}
          role="separator"
          aria-orientation="horizontal"
        />
        <Bottom tab={tab} cursor={cursor} setCursor={setCursor} select={select} />
      </div>
      <EditorStatus tab={tab} pos={pos} />
    </div>
  );
}

function EditorToolbar({ tab, run, end }: { tab: EditorTab; run(mode: RunMode): unknown; end(how: TxEnd): unknown }) {
  const connected = useStore((s) => !!s.servers[tab.registration]);
  const s = useEditors.getState();
  const busy = tab.running || !connected;
  const active = tab.tx !== null;
  return (
    <div className="flex h-8 shrink-0 items-center gap-0.5 border-b border-line bg-panel-2 px-1.5">
      <Tool icon="run" label={t("Execute (F9)")} disabled={busy} onClick={() => run("current")} accent />
      <Tool icon="run-cursor" label={t("Execute to Cursor (Alt+F9)")} disabled={busy} onClick={() => run("to_cursor")} />
      <Tool icon="run-all" label={t("Execute All (Shift+F9)")} disabled={busy} onClick={() => run("all")} />
      <span className="mx-1 h-4 w-px bg-line" />
      <Tool icon="commit" label={t("Commit (Ctrl+Alt+C)")} disabled={busy || !active} onClick={() => end("commit")} />
      <Tool icon="rollback" label={t("Rollback (Ctrl+Alt+R)")} disabled={busy || !active} onClick={() => end("rollback")} />
      <span className="mx-1 h-4 w-px bg-line" />
      <label className="flex items-center gap-1.5 text-muted" title={active ? t("Applies to the next transaction") : undefined}>
        {t("Isolation")}
        <select
          className="h-6 rounded border border-line bg-panel px-1 text-fg disabled:text-faint"
          value={tab.txOptions.isolation}
          disabled={active}
          onChange={(e) => s.setTxOptions(tab.id, { isolation: e.target.value as EditorTab["txOptions"]["isolation"] })}
        >
          {isolations.map((i) => (
            <option key={i.value} value={i.value}>
              {t(i.label)}
            </option>
          ))}
        </select>
      </label>
      <label className="ml-2 flex items-center gap-1 text-muted">
        <input
          type="checkbox"
          checked={tab.txOptions.readOnly}
          disabled={active}
          onChange={(e) => s.setTxOptions(tab.id, { readOnly: e.target.checked })}
        />
        {t("Read only")}
      </label>
      <label className="ml-2 flex items-center gap-1 text-muted">
        <input
          type="checkbox"
          checked={tab.txOptions.lockWait.mode === "wait"}
          disabled={active}
          onChange={(e) => s.setTxOptions(tab.id, { lockWait: { mode: e.target.checked ? "wait" : "no_wait" } })}
        />
        {t("Wait on locks")}
      </label>
      <span className="flex-1" />
      {tab.running && (
        <span className="flex items-center gap-1.5 pr-1 text-muted" role="status">
          <span className="size-2 animate-pulse rounded-full bg-accent" />
          {t("Running…")}
        </span>
      )}
    </div>
  );
}

function Tool(props: { icon: string; label: string; disabled?: boolean; accent?: boolean; onClick(): unknown }) {
  return (
    <button
      title={props.label}
      aria-label={props.label}
      disabled={props.disabled}
      onClick={() => void props.onClick()}
      className={`grid size-7 place-items-center rounded hover:bg-hover disabled:text-faint disabled:hover:bg-transparent ${
        props.accent ? "text-ok" : "text-fg"
      }`}
    >
      <Icon name={props.icon} size={15} />
    </button>
  );
}

const bottomTabs: { id: BottomTab; label: string }[] = [
  { id: "results", label: "Results" },
  { id: "messages", label: "Messages" },
  { id: "columns", label: "Query Columns" },
  { id: "plan", label: "Plan" },
  { id: "performance", label: "Performance Analysis" },
];

function Bottom(props: {
  tab: EditorTab;
  cursor: GridCursor;
  setCursor(c: GridCursor): void;
  select(range: { from: number; to: number }, error?: boolean): void;
}) {
  const { tab } = props;
  const errors = tab.messages.filter((m) => m.level === "error").length;
  let pane: ReactNode;
  switch (tab.bottom) {
    case "results":
      pane = <Results {...props} />;
      break;
    case "messages":
      pane = <Messages tab={tab} select={props.select} />;
      break;
    case "columns":
      pane = <Columns tab={tab} />;
      break;
    case "plan":
      pane = <Plan tab={tab} />;
      break;
    case "performance":
      pane = <PerformancePane tab={tab} />;
      break;
  }
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex h-7 shrink-0 items-stretch border-b border-line bg-panel-2" role="tablist">
        {bottomTabs.map((b) => (
          <button
            key={b.id}
            role="tab"
            aria-selected={tab.bottom === b.id}
            onClick={() => useEditors.getState().setBottom(tab.id, b.id)}
            className={`border-r border-line px-3 ${
              tab.bottom === b.id ? "bg-panel text-fg shadow-[inset_0_-2px_0_var(--accent)]" : "text-muted hover:bg-hover"
            }`}
          >
            {t(b.label)}
            {b.id === "messages" && errors > 0 && <span className="ml-1.5 text-danger">{errors}</span>}
          </button>
        ))}
      </div>
      <div className="min-h-0 flex-1">{pane}</div>
    </div>
  );
}

function Empty({ children }: { children: ReactNode }) {
  return <div className="grid h-full place-items-center p-4 text-center text-muted">{children}</div>;
}

function Results({ tab, cursor, setCursor }: { tab: EditorTab; cursor: GridCursor; setCursor(c: GridCursor): void }) {
  const outcome = tab.outcome;
  const fetchMore = useCallback((max: number) => useEditors.getState().fetchMore(tab.id, max), [tab.id]);
  if (!outcome) return <Empty>{t("Execute a statement with F9 or Ctrl+Enter.")}</Empty>;
  const { executed, result } = outcome;

  if (!result) {
    if (executed.rows.length > 0) return <StaticRows columns={executed.columns.map((c) => c.name)} rows={executed.rows} />;
    return <Empty>{t("The statement returned no rows.")}</Empty>;
  }

  const fetchAll = async () => {
    await fetchMore(Number.MAX_SAFE_INTEGER);
    const rows = useEditors.getState().tabs[tab.id]?.outcome?.result?.rows ?? 0;
    setCursor({ row: Math.max(0, rows - 1), col: cursor.col });
  };
  const go = (row: number) => setCursor({ row: Math.max(0, Math.min(row, result.rows - 1)), col: cursor.col });

  return (
    <div className="flex h-full flex-col">
      <div className="flex h-7 shrink-0 items-center gap-0.5 border-b border-line px-1.5 text-muted">
        <Nav icon="first" label={t("First Record")} disabled={cursor.row === 0} onClick={() => go(0)} />
        <Nav icon="prev" label={t("Prior Record")} disabled={cursor.row === 0} onClick={() => go(cursor.row - 1)} />
        <Nav icon="next" label={t("Next Record")} disabled={cursor.row >= result.rows - 1} onClick={() => go(cursor.row + 1)} />
        <Nav
          icon="last"
          label={t("Last Record (fetches all)")}
          disabled={tab.running || (result.eof && cursor.row >= result.rows - 1)}
          onClick={() => (result.eof ? go(result.rows - 1) : fetchAll())}
        />
        <Nav icon="fetch-all" label={t("Fetch All")} disabled={tab.running || result.eof} onClick={fetchAll} />
        <span className="ml-2 tabular-nums">
          {result.rows === 0
            ? t("No records")
            : t("Record {n} of {total}", {
                n: (cursor.row + 1).toLocaleString(),
                total: result.eof ? result.rows.toLocaleString() : `${result.rows.toLocaleString()}+`,
              })}
        </span>
        {result.endedEarly && <span className="ml-2 text-faint">{t("(the transaction ended before all rows were fetched)")}</span>}
      </div>
      <div className="min-h-0 flex-1">
        <ResultGrid result={result} columns={executed.columns} cursor={cursor} onCursor={setCursor} onFetchMore={fetchMore} />
      </div>
    </div>
  );
}

function Nav(props: { icon: string; label: string; disabled?: boolean; onClick(): unknown }) {
  return (
    <button
      title={props.label}
      aria-label={props.label}
      disabled={props.disabled}
      onClick={() => void props.onClick()}
      className="grid size-6 place-items-center rounded text-fg hover:bg-hover disabled:text-faint disabled:hover:bg-transparent"
    >
      <Icon name={props.icon} size={13} />
    </button>
  );
}

/** A handful of rows held here: an EXECUTE PROCEDURE's outputs */
function StaticRows({ columns, rows }: { columns: string[]; rows: (string | null)[][] }) {
  return (
    <div className="scroll-thin h-full overflow-auto">
      <table className="border-collapse font-mono text-[12px]">
        <thead>
          <tr>
            {columns.map((c, i) => (
              <th key={i} className="sticky top-0 border-r border-b border-line bg-panel-2 px-2 text-left font-sans font-semibold text-muted">
                {c}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {rows.map((r, i) => (
            <tr key={i}>
              {r.map((v, j) => (
                <td key={j} className="border-r border-b border-line/60 px-2">
                  {v === null ? <span className="text-faint italic">{t("<null>")}</span> : v}
                </td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

function Messages({ tab, select }: { tab: EditorTab; select(range: { from: number; to: number }, error?: boolean): void }) {
  const end = useRef<HTMLDivElement>(null);
  useEffect(() => {
    end.current?.scrollIntoView({ block: "end" });
  }, [tab.messages.length]);
  if (tab.messages.length === 0) return <Empty>{t("No messages yet.")}</Empty>;
  return (
    <div className="scroll-thin h-full overflow-auto py-1 font-mono text-[12px]">
      {tab.messages.map((m, i) => (
        <div
          key={i}
          onClick={() => m.range && select(m.range, m.level === "error")}
          className={`flex gap-3 px-2 py-0.5 whitespace-pre-wrap ${m.range ? "cursor-pointer hover:bg-hover" : ""} ${
            m.level === "error" ? "text-danger" : ""
          }`}
        >
          <span className="shrink-0 text-faint tabular-nums">{new Date(m.at).toLocaleTimeString()}</span>
          <span className="min-w-0 select-text">{m.text}</span>
        </div>
      ))}
      <div ref={end} />
    </div>
  );
}

function Columns({ tab }: { tab: EditorTab }) {
  const columns = tab.outcome?.executed.columns ?? [];
  if (columns.length === 0) return <Empty>{t("The last statement has no output columns.")}</Empty>;
  return (
    <div className="scroll-thin h-full overflow-auto">
      <table className="w-full border-collapse text-[12px]">
        <thead>
          <tr className="text-left text-muted">
            {["#", t("Name"), t("Type"), t("Nullable"), t("Table"), t("Field")].map((h) => (
              <th key={h} className="sticky top-0 border-b border-line bg-panel-2 px-2 py-1 font-semibold">
                {h}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {columns.map((c, i) => (
            <tr key={i} className="border-b border-line/60">
              <td className="px-2 text-right text-faint tabular-nums">{i + 1}</td>
              <td className="px-2 font-semibold">{c.name}</td>
              <td className="px-2 font-mono">{c.typeName}</td>
              <td className="px-2">{c.nullable ? t("Yes") : t("No")}</td>
              <td className="px-2">{c.relation}</td>
              <td className="px-2">{c.field}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

function Plan({ tab }: { tab: EditorTab }) {
  const e = tab.outcome?.executed;
  if (!e?.plan && !e?.explainedPlan) return <Empty>{t("The last statement has no plan.")}</Empty>;
  return (
    <div className="scroll-thin h-full overflow-auto p-2 font-mono text-[12px] select-text">
      {e.plan && <pre className="whitespace-pre-wrap">{e.plan.trim()}</pre>}
      {e.explainedPlan && (
        <>
          <div className="mt-3 mb-1 font-sans font-semibold text-muted">{t("Explained plan")}</div>
          <pre className="whitespace-pre-wrap">{e.explainedPlan.trim()}</pre>
        </>
      )}
    </div>
  );
}

function PerformancePane({ tab }: { tab: EditorTab }) {
  const outcome = tab.outcome;
  if (!outcome) return <Empty>{t("Execute a statement to see what it cost.")}</Empty>;
  const { executed, result } = outcome;
  const perf = result ? result.performance : executed.performance;
  const kb = (b: number) => `${Math.round(b / 1024).toLocaleString()} KB`;
  const n = (v: number) => v.toLocaleString();
  const rows: [string, string][] = [
    [t("Prepare"), ms(executed.prepareMs)],
    [t("Execute"), ms(executed.executeMs)],
  ];
  if (result) rows.push([t("Fetch"), `${ms(result.fetchMs)} · ${t("{n} row(s)", { n: n(result.rows) })}`]);
  if (perf) {
    rows.push(
      [t("Reads"), n(perf.reads)],
      [t("Writes"), n(perf.writes)],
      [t("Fetches"), n(perf.fetches)],
      [t("Marks"), n(perf.marks)],
      [t("Current memory"), kb(perf.currentMemory)],
      [t("Max memory"), kb(perf.maxMemory)],
      [t("Buffers"), `${n(perf.buffers)} × ${n(perf.pageSize)} ${t("bytes")}`],
    );
  }
  return (
    <div className="scroll-thin flex h-full gap-6 overflow-auto p-2 text-[12px]">
      <table className="h-min shrink-0">
        <tbody>
          {rows.map(([k, v]) => (
            <tr key={k}>
              <td className="pr-4 text-muted">{k}</td>
              <td className="text-right tabular-nums">{v}</td>
            </tr>
          ))}
        </tbody>
      </table>
      {!perf ? (
        <p className="text-muted">
          {result && !result.eof
            ? t("Fetch all the rows to see the statement's reads and writes.")
            : t("Counters are measured when a single statement runs.")}
        </p>
      ) : perf.tables.length > 0 ? (
        <table className="h-min border-collapse">
          <thead>
            <tr className="text-muted">
              {[t("Table"), t("Indexed reads"), t("Non-indexed reads"), t("Inserts"), t("Updates"), t("Deletes"), t("Backouts"), t("Purges"), t("Expunges")].map(
                (h, i) => (
                  <th key={h} className={`border-b border-line px-2 py-0.5 font-semibold ${i ? "text-right" : "text-left"}`}>
                    {h}
                  </th>
                ),
              )}
            </tr>
          </thead>
          <tbody>
            {perf.tables.map((r) => (
              <tr key={r.table} className="border-b border-line/60">
                <td className="px-2 font-semibold">{r.table}</td>
                {[r.indexedReads, r.nonIndexedReads, r.inserts, r.updates, r.deletes, r.backouts, r.purges, r.expunges].map((v, i) => (
                  <td key={i} className={`px-2 text-right tabular-nums ${i === 1 && v > 0 ? "text-danger" : ""} ${v === 0 ? "text-faint" : ""}`}>
                    {n(v)}
                  </td>
                ))}
              </tr>
            ))}
          </tbody>
        </table>
      ) : (
        <p className="text-muted">{t("No table was read or written.")}</p>
      )}
    </div>
  );
}

function EditorStatus({ tab, pos }: { tab: EditorTab; pos: { line: number; col: number } }) {
  const isolation = isolations.find((i) => i.value === tab.txOptions.isolation)?.label ?? "";
  const result = tab.outcome?.result;
  return (
    <div className="flex h-6 shrink-0 items-center gap-3 border-t border-line bg-panel-2 px-2 text-[11.5px] text-muted">
      <span className="tabular-nums">
        {t("Ln {line}, Col {col}", { line: pos.line, col: pos.col })}
      </span>
      <span className="h-3 w-px bg-line" />
      <span className="flex items-center gap-1.5">
        <span className={`size-1.5 rounded-full ${tab.tx !== null ? "bg-accent" : "bg-faint"}`} />
        {tab.tx !== null
          ? t("Transaction active · {isolation}{ro}", { isolation: t(isolation), ro: tab.txOptions.readOnly ? ` · ${t("read only")}` : "" })
          : t("No active transaction")}
      </span>
      {result && (
        <>
          <span className="h-3 w-px bg-line" />
          <span className="tabular-nums">
            {result.eof
              ? t("{n} row(s) fetched", { n: result.rows.toLocaleString() })
              : t("{n} row(s) fetched, more available", { n: result.rows.toLocaleString() })}
          </span>
        </>
      )}
    </div>
  );
}
