// The SQL editor: one tab per editor, each with its own transaction.
// The text on top, what the last statement did below.
import { EditorState, Prec, StateEffect, StateField } from "@codemirror/state";
import { Decoration, EditorView, keymap, type DecorationSet } from "@codemirror/view";
import type { IDockviewPanelProps } from "dockview-react";
import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { api, errorText, type TxEnd } from "../api";
import { drafts, isolations, MAX_PAGES, pageOf, readers, useEditors, type BottomTab, type EditorTab, type RunMode } from "../editors";
import { prepareCompletion } from "../complete";
import { t } from "../i18n";
import { Icon } from "../icons";
import { formatEditor } from "../format";
import { formatLabel, pageName, saveEditor } from "../scripts";
import { request } from "../pending";
import { sessionChanged } from "../session";
import { sqlExtensions } from "../sqlView";
import { useStore } from "../store";
import { useContextMenu } from "./ContextMenu";
import type { GridCursor } from "./ResultGrid";
import { HistoryPane } from "./HistoryPane";
import { noEdits, toChanges } from "../rowEdits";
import { PerformancePane } from "./PerformancePane";
import { Empty, Results, type ResultsEdit } from "./Results";

/** Editor states by page, so a page put behind another, or a panel that
 *  remounts, keeps its text, its selection and its undo */
const saved = new Map<string, EditorState>();
/** The page each mounted editor's view shows, by tab */
const shown = new Map<string, string>();
/** What the run keys call, by tab: the mounted panel's callbacks. Kept out
 *  of the saved state, which outlives a mount. */
const runners = new Map<string, { run(mode: RunMode | "script"): boolean; end(how: TxEnd): boolean }>();

/** Esc stops what the tab runs; otherwise it is the editor's (closing a
 *  completion, the search panel) */
function cancelKey(id: string) {
  return () => {
    const tab = useEditors.getState().tabs[id];
    if (!tab?.running && !tab?.fetching) return false;
    void useEditors.getState().cancel(id);
    return true;
  };
}

/** The modifier a shortcut is said with */
const ALT = /Mac|iPhone|iPad/.test(navigator.platform) ? "Option" : "Alt";

/** The mounted editors, by tab: what Format lays out */
const views = new Map<string, EditorView>();
const format = (id: string) => {
  const view = views.get(id);
  if (view) void formatEditor(view);
  return !!view;
};

/** A closed editor's pages go with it */
export function forgetEditor(id: string) {
  for (const page of saved.keys()) if (page.startsWith(`${id}:`)) saved.delete(page);
  shown.delete(id);
  runners.delete(id);
}

/** Another page, to the right (`by` 1) or to the left (-1), round at the ends */
function turnPage(id: string, by: number): boolean {
  const tab = useEditors.getState().tabs[id];
  if (!tab || tab.pages.length < 2) return false;
  const at = tab.pages.findIndex((p) => p.id === tab.page);
  useEditors.getState().showPage(id, tab.pages[(at + by + tab.pages.length) % tab.pages.length].id);
  return true;
}

const newPage = (id: string) => {
  useEditors.getState().addPage(id);
  return true;
};

function runKeys(id: string) {
  const run = (mode: RunMode | "script") => () => runners.get(id)?.run(mode) ?? false;
  return Prec.highest(
    keymap.of([
      { key: "F9", run: run("current") },
      { key: "Mod-Enter", run: run("current") },
      { key: "Shift-F9", run: run("all") },
      { key: "Mod-Shift-Enter", run: run("all") },
      { key: "Alt-F9", run: run("to_cursor") },
      { key: "Mod-Alt-Enter", run: run("to_cursor") },
      { key: "Ctrl-F9", run: run("script") },
      { key: "Mod-s", run: () => (void saveEditor(id), true) },
      { key: "Mod-Shift-s", run: () => (void saveEditor(id, true), true) },
      // The key editors elsewhere have; IBExpert's is `formatKey`
      { key: "Mod-Shift-f", run: () => format(id) },
      { key: "Escape", run: cancelKey(id) },
      // The pages, as a browser's tabs
      { key: "Mod-t", run: () => newPage(id) },
      { key: "Ctrl-Tab", run: () => turnPage(id, 1) },
      { key: "Ctrl-Shift-Tab", run: () => turnPage(id, -1) },
      { key: "Ctrl-PageDown", run: () => turnPage(id, 1) },
      { key: "Ctrl-PageUp", run: () => turnPage(id, -1) },
    ]),
  );
}

/**
 * Alt+F, IBExpert's key for the formatter, taken by the key pressed and not
 * by the character it types: on a Mac, Option+F types "ƒ", and a keymap
 * leaves Option alone with a letter to the typing.
 */
function formatKey(id: string) {
  return Prec.highest(
    EditorView.domEventHandlers({
      keydown(event) {
        if (event.code !== "KeyF" || !event.altKey || event.ctrlKey || event.metaKey || event.shiftKey) return false;
        event.preventDefault();
        return format(id);
      },
    }),
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

  /** A place to select once the page it is in is in front */
  const pending = useRef<{ range: { from: number; to: number }; error: boolean } | null>(null);
  const select = useCallback((range: { from: number; to: number; page?: string }, error = false) => {
    const v = view.current;
    if (!v) return;
    const editors = useEditors.getState();
    if (range.page && range.page !== shown.get(id) && editors.tabs[id]?.pages.some((p) => p.id === range.page)) {
      pending.current = { range, error };
      editors.showPage(id, range.page);
      return;
    }
    const len = v.state.doc.length;
    const r = { from: Math.min(range.from, len), to: Math.min(range.to, len) };
    v.dispatch({
      selection: { anchor: r.from, head: r.to },
      effects: [markError.of(error ? r : null), EditorView.scrollIntoView(r.from, { y: "center" })],
    });
    v.focus();
  }, [id]);

  const insert = useCallback((text: string) => {
    const v = view.current;
    if (!v) return;
    v.dispatch({ ...v.state.replaceSelection(text), scrollIntoView: true, userEvent: "input.paste" });
    v.focus();
  }, []);

  const run = useCallback(
    (mode: RunMode | "script") => {
      const v = view.current;
      if (!v) return false;
      const sel = v.state.selection.main;
      v.dispatch({ effects: markError.of(null) });
      const input = { text: v.state.doc.toString(), from: sel.from, to: sel.to };
      const editors = useEditors.getState();
      // A failure is shown in the page that ran, whichever is in front by then
      const page = shown.get(id);
      const failed = (r: { from: number; to: number }) => select({ ...r, page }, true);
      if (mode === "script") void editors.runScript(id, input, failed);
      else void editors.run(id, mode, input, failed);
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

  /** A page's state: the one it was left with, or a new one with its text */
  const stateOf = useCallback(
    (page: string) => {
      let state = saved.get(page);
      if (!state) {
        state = EditorState.create({
          doc: drafts.get(page) ?? "",
          extensions: [
            runKeys(id),
            formatKey(id),
            errorField,
            // Typing makes the text other than its file's, and the session's
            EditorView.updateListener.of((update) => {
              if (!update.docChanged) return;
              useEditors.getState().edited(id);
              sessionChanged();
            }),
            ...sqlExtensions(() => useEditors.getState().tabs[id]?.registration),
          ],
        });
        saved.set(page, state);
        drafts.delete(page);
        // From the view while it shows the page, from its kept state else
        readers.set(page, () => (shown.get(id) === page && view.current ? view.current.state : saved.get(page))?.doc.toString() ?? "");
      }
      return state;
    },
    [id],
  );

  const showPos = (v: EditorView) => {
    const head = v.state.selection.main.head;
    const line = v.state.doc.lineAt(head);
    setPos({ line: line.number, col: head - line.from + 1 });
  };

  useEffect(() => {
    if (!host.current) return;
    runners.set(id, { run, end });
    const page = useEditors.getState().tabs[id]?.page;
    if (!page) return;
    const v = new EditorView({
      state: stateOf(page),
      parent: host.current,
      dispatchTransactions(trs, v) {
        v.update(trs);
        showPos(v);
      },
    });
    shown.set(id, page);
    view.current = v;
    views.set(id, v);
    showPos(v);
    v.focus();
    return () => {
      const at = shown.get(id);
      if (at) saved.set(at, v.state);
      v.destroy();
      view.current = null;
      views.delete(id);
    };
  }, [id, run, end, stateOf]);

  // Another page in front: the view shows it, as it was left
  const front = tab?.page;
  useEffect(() => {
    const v = view.current;
    const was = shown.get(id);
    if (!v || !front || was === front) return;
    if (was && useEditors.getState().tabs[id]?.pages.some((p) => p.id === was)) saved.set(was, v.state);
    v.setState(stateOf(front));
    shown.set(id, front);
    showPos(v);
    const go = pending.current;
    pending.current = null;
    if (go) select(go.range, go.error);
    else v.focus();
  }, [id, front, stateOf, select]);

  // A closed page's state goes with it
  const pages = tab?.pages;
  useEffect(() => {
    if (!pages) return;
    for (const page of saved.keys()) if (page.startsWith(`${id}:`) && !pages.some((p) => p.id === page)) saved.delete(page);
  }, [id, pages]);

  // A new result starts at its first row; the same rows read again after
  // a change stay where they were, as far as there still are rows. A new
  // one is shown, the bottom restored if it was minimized.
  const resultId = tab?.outcome?.result?.id;
  useEffect(() => {
    const now = useEditors.getState().tabs[id];
    const outcome = now?.outcome;
    const rows = outcome?.result?.rows ?? 0;
    setCursor((c) => (outcome?.again ? { row: Math.max(0, Math.min(c.row, rows - 1)), col: c.col } : { row: 0, col: 0 }));
    if (resultId !== undefined && !outcome?.again && now?.minimized) useEditors.getState().setMinimized(id, false);
  }, [resultId, id]);

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

  // IBExpert's Ctrl+Alt+C and Ctrl+Alt+R, wherever in the tab the keyboard
  // is, the grid too. By the key pressed and not the character it types:
  // on Windows Ctrl+Alt is AltGr, and AltGr+C types "₢" on a Brazilian
  // keyboard, which a keymap would take for some other key.
  const endKeys = (e: React.KeyboardEvent) => {
    if (!e.altKey || !(e.ctrlKey || e.metaKey) || e.shiftKey) return;
    const how = e.code === "KeyC" ? "commit" : e.code === "KeyR" ? "rollback" : null;
    if (!how) return;
    e.preventDefault();
    e.stopPropagation();
    end(how);
  };

  return (
    <div className="flex h-full flex-col bg-panel" onKeyDownCapture={endKeys}>
      <EditorToolbar tab={tab} run={run} end={end} />
      <PageStrip tab={tab} />
      <div ref={body} className="flex min-h-0 flex-1 flex-col">
        <div
          ref={host}
          className={`min-h-0 overflow-hidden ${tab.minimized ? "flex-1" : ""}`}
          style={tab.minimized ? undefined : { height: `${split * 100}%` }}
        />
        {!tab.minimized && (
          <div
            className="h-[5px] shrink-0 cursor-row-resize border-y border-line bg-panel-2 hover:bg-accent/40"
            onMouseDown={startSplit}
            role="separator"
            aria-orientation="horizontal"
          />
        )}
        <Bottom tab={tab} cursor={cursor} setCursor={setCursor} select={select} insert={insert} />
      </div>
      <EditorStatus tab={tab} pos={pos} />
    </div>
  );
}

/** The editor's pages, as a browser's tabs: the one in front is what runs.
 *  A double click on one, or its menu, renames it. */
function PageStrip({ tab }: { tab: EditorTab }) {
  const editors = useEditors.getState();
  const showMenu = useContextMenu((m) => m.show);
  const [renaming, setRenaming] = useState<string | null>(null);
  const full = tab.pages.length >= MAX_PAGES;
  const close = (page: string) => request({ kind: "closePage", tab: tab.id, page });
  const menu = (e: React.MouseEvent, page: string) => {
    e.preventDefault();
    showMenu(e.clientX, e.clientY, [
      { label: t("Rename…"), icon: "edit", onSelect: () => setRenaming(page) },
      { label: t("New Page (Ctrl+T)"), icon: "plus", disabled: full, onSelect: () => editors.addPage(tab.id) },
      "separator",
      { label: t("Close the Page"), disabled: tab.pages.length <= 1, onSelect: () => close(page) },
    ]);
  };
  return (
    <div
      className="scroll-thin flex h-7 shrink-0 items-stretch overflow-x-auto border-b border-line bg-panel-2"
      role="tablist"
      aria-label={t("Pages")}
      // On the strip's empty part, as on a browser's
      onDoubleClick={(e) => e.target === e.currentTarget && editors.addPage(tab.id)}
    >
      {tab.pages.map((page) => {
        const front = page.id === tab.page;
        const name = pageName(page);
        return (
          <div
            key={page.id}
            role="tab"
            aria-selected={front}
            title={page.file ?? name}
            onDoubleClick={() => setRenaming(page.id)}
            onContextMenu={(e) => menu(e, page.id)}
            onMouseDown={(e) => {
              // The middle button closes it, as on a browser's
              if (e.button === 1 && tab.pages.length > 1) {
                e.preventDefault();
                close(page.id);
              } else if (e.button === 0) editors.showPage(tab.id, page.id);
            }}
            className={`group flex max-w-[14rem] shrink-0 cursor-default items-center gap-1 border-r border-line pr-1 pl-2.5 ${
              front ? "bg-panel text-fg shadow-[inset_0_-2px_0_var(--accent)]" : "text-muted hover:bg-hover"
            }`}
          >
            {renaming === page.id ? (
              <PageName
                page={name}
                done={(title) => {
                  setRenaming(null);
                  // Its own name typed back is no name of its own: it follows its file again
                  if (title !== null) editors.renamePage(tab.id, page.id, title.trim() === pageName({ ...page, title: null }) ? "" : title);
                }}
              />
            ) : (
              <span className="truncate">{name}</span>
            )}
            {page.changed && <span className="text-accent">•</span>}
            {tab.pages.length > 1 && (
              <button
                title={t("Close the page")}
                aria-label={t("Close {name}", { name })}
                onMouseDown={(e) => e.stopPropagation()}
                onClick={() => close(page.id)}
                className={`grid size-4 place-items-center rounded text-[13px] leading-none text-muted hover:bg-hover hover:text-fg ${
                  front ? "" : "opacity-0 group-hover:opacity-100"
                }`}
              >
                ×
              </button>
            )}
          </div>
        );
      })}
      <button
        title={full ? t("An editor has at most {n} pages", { n: MAX_PAGES }) : t("New Page (Ctrl+T)")}
        aria-label={t("New Page")}
        disabled={full}
        onClick={() => editors.addPage(tab.id)}
        className="grid w-7 shrink-0 place-items-center text-muted hover:bg-hover hover:text-fg disabled:text-faint disabled:hover:bg-transparent"
      >
        <Icon name="plus" size={13} />
      </button>
    </div>
  );
}

/** A page's name being typed: Enter or leaving it keeps it, Escape does not
 *  (`done(null)`). Left blank, the page goes back to the name it had before
 *  it was given one. */
function PageName({ page, done }: { page: string; done(title: string | null): void }) {
  const [text, setText] = useState(page);
  const finished = useRef(false);
  const finish = (title: string | null) => {
    if (finished.current) return;
    finished.current = true;
    done(title);
  };
  return (
    <input
      autoFocus
      value={text}
      aria-label={t("Page name")}
      onFocus={(e) => e.currentTarget.select()}
      onChange={(e) => setText(e.target.value)}
      onMouseDown={(e) => e.stopPropagation()}
      onDoubleClick={(e) => e.stopPropagation()}
      onKeyDown={(e) => {
        // The editor's keys are not this field's
        e.stopPropagation();
        if (e.key === "Enter") finish(text);
        else if (e.key === "Escape") finish(null);
      }}
      onBlur={() => finish(text)}
      className="h-5 w-32 rounded border border-accent bg-panel px-1 text-fg outline-none"
    />
  );
}

function EditorToolbar(props: { tab: EditorTab; run(mode: RunMode | "script"): unknown; end(how: TxEnd): unknown }) {
  const { tab, run, end } = props;
  const showMenu = useContextMenu((m) => m.show);
  const connected = useStore((s) => !!s.servers[tab.registration]);
  useEffect(() => {
    if (connected) prepareCompletion(tab.registration);
  }, [connected, tab.registration]);
  const scriptMenu = (e: React.MouseEvent) => {
    const rect = e.currentTarget.getBoundingClientRect();
    const { stopOnError, autoDdl } = tab.scriptOptions;
    const set = useEditors.getState().setScriptOptions;
    showMenu(rect.left, rect.bottom + 2, [
      { label: t("Stop at the First Error"), icon: stopOnError ? "commit" : undefined, onSelect: () => set(tab.id, { stopOnError: !stopOnError }) },
      { label: t("Commit After Each DDL Statement"), icon: autoDdl ? "commit" : undefined, onSelect: () => set(tab.id, { autoDdl: !autoDdl }) },
    ]);
  };
  const saveMenu = (e: React.MouseEvent) => {
    const rect = e.currentTarget.getBoundingClientRect();
    showMenu(rect.left, rect.bottom + 2, [
      { label: t("Save"), icon: "save", onSelect: () => void saveEditor(tab.id) },
      { label: t("Save As… (Ctrl+Shift+S)"), onSelect: () => void saveEditor(tab.id, true) },
    ]);
  };
  const formatMenu = (e: React.MouseEvent) => {
    const rect = e.currentTarget.getBoundingClientRect();
    showMenu(rect.left, rect.bottom + 2, [
      { label: t("Format ({key}+F)", { key: ALT }), icon: "format", onSelect: () => void format(tab.id) },
      { label: t("Formatter Options…"), onSelect: () => useStore.getState().openDialog({ type: "formatter" }) },
    ]);
  };
  const s = useEditors.getState();
  const busy = tab.running || !connected;
  const active = tab.tx !== null;
  return (
    <div className="flex h-8 shrink-0 items-center gap-0.5 border-b border-line bg-panel-2 px-1.5">
      <Tool icon="run" label={t("Execute (F9)")} disabled={busy} onClick={() => run("current")} accent />
      <Tool icon="run-cursor" label={t("Execute to Cursor (Alt+F9)")} disabled={busy} onClick={() => run("to_cursor")} />
      <Tool icon="run-all" label={t("Execute All (Shift+F9)")} disabled={busy} onClick={() => run("all")} />
      <Tool icon="script" label={t("Execute as Script (Ctrl+F9): every statement, errors logged")} disabled={busy} onClick={() => run("script")} />
      <button
        title={t("Script Options")}
        aria-label={t("Script Options")}
        onClick={scriptMenu}
        className="-ml-0.5 grid h-7 w-3.5 place-items-center rounded text-muted hover:bg-hover"
      >
        <Icon name="options" size={10} />
      </button>
      <span className="mx-1 h-4 w-px bg-line" />
      <Tool icon="save" label={t("Save (Ctrl+S)")} onClick={() => saveEditor(tab.id)} />
      <button
        title={t("Save Options")}
        aria-label={t("Save Options")}
        onClick={saveMenu}
        className="-ml-0.5 grid h-7 w-3.5 place-items-center rounded text-muted hover:bg-hover"
      >
        <Icon name="options" size={10} />
      </button>
      <Tool
        icon="format"
        label={t("Format the selection, or all of the text ({key}+F)", { key: ALT })}
        onClick={() => format(tab.id)}
      />
      <button
        title={t("Formatter Options")}
        aria-label={t("Formatter Options")}
        onClick={formatMenu}
        className="-ml-0.5 grid h-7 w-3.5 place-items-center rounded text-muted hover:bg-hover"
      >
        <Icon name="options" size={10} />
      </button>
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
      {(tab.running || tab.fetching) && (
        <span className="flex items-center gap-1.5 pr-1 text-muted" role="status">
          <span className="size-2 animate-pulse rounded-full bg-accent" />
          {tab.script && tab.script.statements > 0
            ? t("Statement {at} of {statements}{failed}", {
                at: tab.script.at.toLocaleString(),
                statements: tab.script.statements.toLocaleString(),
                failed: tab.script.failed > 0 ? ` · ${t("{n} failed", { n: tab.script.failed.toLocaleString() })}` : "",
              })
            : tab.running
              ? t("Running…")
              : t("Fetching…")}
          <button
            onClick={() => void s.cancel(tab.id)}
            title={t("Stop (Esc)")}
            className="ml-1 flex h-6 items-center gap-1 rounded border border-line bg-panel px-2 text-fg hover:bg-hover"
          >
            <Icon name="stop" size={12} />
            {t("Stop")}
          </button>
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
  { id: "history", label: "History" },
  { id: "columns", label: "Query Columns" },
  { id: "plan", label: "Plan" },
  { id: "performance", label: "Performance Analysis" },
];

/** Shows the script that makes the grid's changes the table's, to run in
 *  the editor's transaction when it has one */
async function applyRows(tab: EditorTab, table: string) {
  const s = useStore.getState();
  try {
    const script = await api.dataChangesScript(tab.registration, table, toChanges(tab.edits));
    const tx = useEditors.getState().tabs[tab.id]?.tx ?? undefined;
    s.openDialog({
      type: "compile",
      registration: tab.registration,
      title: t("Apply Changes: {name}", { name: table }),
      script,
      verb: tx === undefined ? t("Run and Commit") : t("Run"),
      tx,
      done: () => {
        const editors = useEditors.getState();
        if (tx !== undefined) editors.note(tab.id, t("The changes to {name} ran in this editor's transaction: commit it to keep them.", { name: table }));
        editors.setEdits(tab.id, noEdits);
        void editors.again(tab.id);
      },
    });
  } catch (e) {
    s.setNotice(errorText(e));
  }
}

/** How the rows of the tab's select are changed in the grid, when they can be */
function rowsEdit(tab: EditorTab): ResultsEdit | undefined {
  const edit = tab.outcome?.edit;
  if (!edit || tab.viewer) return undefined;
  return {
    key: edit.key,
    editable: edit.editable,
    names: edit.names,
    edits: tab.edits,
    onEdits: (edits) => useEditors.getState().setEdits(tab.id, edits),
    onApply: () => void applyRows(tab, edit.table),
    onRevert: () => useEditors.getState().setEdits(tab.id, noEdits),
  };
}

function Bottom(props: {
  tab: EditorTab;
  cursor: GridCursor;
  setCursor(c: GridCursor): void;
  select(range: { from: number; to: number }, error?: boolean): void;
  /** Puts text where the editor's cursor or selection is */
  insert(text: string): void;
}) {
  const { tab } = props;
  const errors = tab.messages.filter((m) => m.level === "error").length;
  let pane: ReactNode;
  switch (tab.bottom) {
    case "results":
      pane = <Results {...props} edit={rowsEdit(tab)} />;
      break;
    case "messages":
      pane = <Messages tab={tab} select={props.select} />;
      break;
    case "history":
      pane = <HistoryPane registration={tab.registration} insert={props.insert} />;
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
    <div className={`flex min-h-0 flex-col ${tab.minimized ? "shrink-0 border-t border-line" : "flex-1"}`}>
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
        <span className="flex-1" />
        <button
          title={tab.minimized ? t("Restore") : t("Minimize")}
          aria-label={tab.minimized ? t("Restore") : t("Minimize")}
          aria-expanded={!tab.minimized}
          onClick={() => useEditors.getState().setMinimized(tab.id, !tab.minimized)}
          className="grid w-7 place-items-center text-muted hover:bg-hover hover:text-fg"
        >
          <Icon name={tab.minimized ? "up" : "down"} size={13} />
        </button>
      </div>
      {!tab.minimized && <div className="min-h-0 flex-1">{pane}</div>}
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

function EditorStatus({ tab, pos }: { tab: EditorTab; pos: { line: number; col: number } }) {
  const isolation = isolations.find((i) => i.value === tab.txOptions.isolation)?.label ?? "";
  const result = tab.outcome?.result;
  const page = pageOf(tab);
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
      {page?.file && page.format && (
        <>
          <span className="h-3 w-px bg-line" />
          <span className="min-w-0 truncate" title={page.file}>
            {page.file}
          </span>
          <span className="shrink-0">{formatLabel(page.format)}</span>
        </>
      )}
      {result && (
        <>
          <span className="h-3 w-px bg-line" />
          <span className="shrink-0 tabular-nums">
            {result.eof
              ? t("{n} row(s) fetched", { n: result.rows.toLocaleString() })
              : t("{n} row(s) fetched, more available", { n: result.rows.toLocaleString() })}
          </span>
        </>
      )}
    </div>
  );
}
