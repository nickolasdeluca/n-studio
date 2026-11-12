// An object's viewer: a row of tabs the backend names, each drawn by its
// shape. Nothing here knows what a Firebird table is made of; a row that
// stands for another object opens that object's viewer.
import { closeBrackets, closeBracketsKeymap } from "@codemirror/autocomplete";
import { defaultKeymap, history, historyKeymap, indentWithTab } from "@codemirror/commands";
import { bracketMatching, indentOnInput } from "@codemirror/language";
import { highlightSelectionMatches, searchKeymap } from "@codemirror/search";
import { EditorState, Prec } from "@codemirror/state";
import { EditorView, keymap, lineNumbers } from "@codemirror/view";
import type { IDockviewPanelProps } from "dockview-react";
import { useEffect, useMemo, useRef, useState } from "react";
import { api, errorText, type Action, type CellKind, type GridRow, type GridSection, type ObjectRef, type RowEditing, type TabContent } from "../api";
import { extractMetadata, startAction } from "../actions";
import { openEditor, openObject } from "../dock";
import { useEditors } from "../editors";
import { editorLook, firebirdSql } from "../firebird";
import { t } from "../i18n";
import { Icon, kindColor } from "../icons";
import { useStore } from "../store";
import { editCount, noEdits, toChanges, type RowEdits } from "../rowEdits";
import { useViewers, type Viewer } from "../viewers";
import { useContextMenu } from "./ContextMenu";
import type { GridCursor } from "./ResultGrid";
import { Empty, Results, type ResultsEdit } from "./Results";

export function ObjectViewer({ params, api: panel }: IDockviewPanelProps<{ viewer: string }>) {
  const viewer = useViewers((s) => s.viewers[params.viewer]);
  const connected = useStore((s) => (viewer ? !!s.servers[viewer.registration] : false));

  // What it shows is the connection's: without one, or after a restart of
  // the app, there is nothing to show
  useEffect(() => {
    if (!viewer || !connected) panel.close();
  }, [viewer, connected, panel]);

  if (!viewer) return <div className="h-full bg-panel" />;
  const { tabs } = viewer;
  const active = tabs.data?.find((tab) => tab.id === viewer.active);
  const content = active ? viewer.contents[active.id] : undefined;
  const open = (ref: ObjectRef) => openObject(viewer.registration, ref.kind, ref.name);

  let pane;
  if (tabs.error) pane = <Empty>{<span className="text-danger">{tabs.error}</span>}</Empty>;
  else if (!active || !content || (content.loading && !content.data)) pane = <Empty>{t("Loading…")}</Empty>;
  else if (content.error) pane = <Empty>{<span className="text-danger select-text">{content.error}</span>}</Empty>;
  else if (content.data) pane = <Pane key={active.id} viewer={viewer} content={content.data} open={open} />;

  return (
    <div className="flex h-full flex-col bg-panel">
      <Header viewer={viewer} busy={!!content?.loading} />
      <div className="flex h-7 shrink-0 items-stretch border-b border-line bg-panel-2" role="tablist">
        {tabs.data?.map((tab) => (
          <button
            key={tab.id}
            role="tab"
            aria-selected={tab.id === viewer.active}
            onClick={() => useViewers.getState().show(viewer.id, tab.id)}
            className={`border-r border-line px-3 ${
              tab.id === viewer.active ? "bg-panel text-fg shadow-[inset_0_-2px_0_var(--accent)]" : "text-muted hover:bg-hover"
            }`}
          >
            {t(tab.label)}
            {(viewer.drafts[tab.id] !== undefined || (tab.shape === "data" && editCount(viewer.edits) > 0)) && <span className="ml-1 text-accent" title={t("Changed, not compiled")}>●</span>}
          </button>
        ))}
      </div>
      <div className="min-h-0 flex-1" role="tabpanel">
        {pane}
      </div>
    </div>
  );
}

function Header({ viewer, busy }: { viewer: Viewer; busy: boolean }) {
  const registration = useStore((s) => s.registrations.find((r) => r.id === viewer.registration));
  const kind = useStore((s) => s.trees[viewer.registration]?.data?.find((k) => k.kind === viewer.kind));
  const own = { kind: viewer.kind, name: viewer.name };
  return (
    <div className="flex h-8 shrink-0 items-center gap-2 border-b border-line bg-panel-2 px-2">
      <Icon name={viewer.kind} className={`shrink-0 ${kindColor[viewer.kind] ?? ""}`} />
      <span className="truncate font-semibold select-text">{viewer.name}</span>
      <span className="truncate text-muted">
        {kind ? t(kind.singular) : viewer.kind}
        {registration ? ` · ${registration.name}` : ""}
      </span>
      <span className="flex-1" />
      {busy && <span className="size-2 animate-pulse rounded-full bg-accent" role="status" aria-label={t("Loading…")} />}
      <HeaderButton icon="sql" label={t("Extract Metadata")} onClick={() => void extractMetadata(viewer.registration, [own])} />
      <HeaderButton icon="refresh" label={t("Reload This Tab")} disabled={busy} onClick={() => useViewers.getState().reload(viewer.id)} />
    </div>
  );
}

function HeaderButton(props: { icon: string; label: string; disabled?: boolean; onClick(): unknown }) {
  return (
    <button
      title={props.label}
      aria-label={props.label}
      disabled={props.disabled}
      onClick={() => void props.onClick()}
      className="grid size-6 place-items-center rounded text-fg hover:bg-hover disabled:text-faint disabled:hover:bg-transparent"
    >
      <Icon name={props.icon} size={14} />
    </button>
  );
}

function Pane({ viewer, content, open }: { viewer: Viewer; content: TabContent; open(ref: ObjectRef): void }) {
  switch (content.shape) {
    case "grid":
      return <Grids viewer={viewer} sections={content.sections} open={open} />;
    case "sql":
      return <SqlText viewer={viewer} text={content.text} editable={content.editable} />;
    case "text":
      return <PlainText viewer={viewer} text={content.text} editable={content.editable} />;
    case "data":
      return <Data viewer={viewer} editing={content.edit} />;
  }
}

/** Shows the script that makes the tab's new text the object's, to compile */
async function compileTab(viewer: Viewer, tab: string, text: string) {
  const s = useStore.getState();
  try {
    const script = await api.objectTabScript(viewer.registration, viewer.kind, viewer.name, tab, text);
    s.openDialog({
      type: "compile",
      registration: viewer.registration,
      title: t("Compile {name}", { name: viewer.name }),
      script,
      done: () => {
        const viewers = useViewers.getState();
        viewers.setDraft(viewer.id, tab, null);
        viewers.refresh(viewer.id);
      },
    });
  } catch (e) {
    s.setNotice(errorText(e));
  }
}

const cellClass: Record<CellKind, string> = {
  text: "",
  number: "text-right tabular-nums",
  flag: "text-center text-ok",
  code: "font-mono text-[12px]",
};

/** A tab's grids, one under the other; a lone grid fills the tab */
function Grids({ viewer, sections, open }: { viewer: Viewer; sections: GridSection[]; open(ref: ObjectRef): void }) {
  // The row the cursor is on: [section, row]
  const [cursor, setCursor] = useState<[number, number]>([0, 0]);
  return (
    <div className="scroll-thin h-full overflow-auto">
      {sections.map((section, s) => (
        <Section
          key={s}
          section={section}
          current={cursor[0] === s ? cursor[1] : -1}
          onCurrent={(row) => setCursor([s, row])}
          open={open}
          act={(action, key) => void startAction(viewer, action, key)}
          sticky={sections.length === 1}
        />
      ))}
    </div>
  );
}

export function Section(props: {
  section: GridSection;
  current: number;
  onCurrent(row: number): void;
  open(ref: ObjectRef): void;
  /** Starts an action, on the row with the key when it is a row's */
  act(action: Action, key: string | null): void;
  /** Its header stays in view: only when it is the tab's one grid */
  sticky: boolean;
}) {
  const { section, current, onCurrent, open, act, sticky } = props;
  const showMenu = useContextMenu((m) => m.show);
  const body = useRef<HTMLTableSectionElement>(null);
  const bar = !!section.title || section.actions.length > 0;

  const move = (row: number) => {
    const next = Math.max(0, Math.min(section.rows.length - 1, row));
    onCurrent(next);
    body.current?.children[next]?.scrollIntoView({ block: "nearest" });
  };
  const onKeyDown = (e: React.KeyboardEvent) => {
    const row = section.rows[current];
    const keys: Record<string, () => void> = {
      ArrowDown: () => move(current + 1),
      ArrowUp: () => move(current - 1),
      Home: () => move(0),
      End: () => move(section.rows.length - 1),
      Enter: () => row?.open && open(row.open),
    };
    if (!keys[e.key]) return;
    e.preventDefault();
    keys[e.key]();
  };
  const menu = (e: React.MouseEvent, row: GridRow, index: number) => {
    e.preventDefault();
    onCurrent(index);
    const target = row.open;
    showMenu(e.clientX, e.clientY, [
      {
        label: target ? t("Open {name}", { name: target.name }) : t("Open"),
        icon: "open",
        disabled: !target,
        onSelect: () => target && open(target),
      },
      {
        label: t("Copy Row"),
        icon: "copy",
        onSelect: () => void navigator.clipboard.writeText(row.cells.map((c) => c ?? "").join("\t")),
      },
      ...(section.row_actions.length > 0 ? (["separator"] as const) : []),
      ...section.row_actions.map((action) => ({
        label: t(action.label),
        icon: action.danger ? "trash" : action.form ? "edit" : undefined,
        danger: action.danger,
        disabled: row.key === null,
        onSelect: () => act(action, row.key),
      })),
    ]);
  };

  return (
    <div className={section.title ? "mb-3" : ""}>
      {bar && (
        <div className={`flex h-7 items-center gap-2 border-b border-line bg-panel-2 px-2 ${sticky ? "sticky top-0 z-20" : ""}`}>
          {section.title && (
            <>
              <span className="font-semibold">{t(section.title)}</span>
              <span className="text-faint tabular-nums">{section.rows.length.toLocaleString()}</span>
            </>
          )}
          <div className={`flex items-center gap-1 ${section.title ? "ml-2" : "-ml-1"}`}>
            {section.actions.map((action) => (
              <TextButton
                key={action.id}
                icon={action.form ? (action.id.startsWith("add_") ? "plus" : "edit") : "run"}
                label={t(action.label)}
                onClick={() => act(action, null)}
              />
            ))}
          </div>
          {section.row_actions.length > 0 && section.rows.length > 0 && (
            <span className="ml-auto truncate text-faint">{t("Right-click a row for more.")}</span>
          )}
        </div>
      )}
      {section.rows.length === 0 ? (
        <p className="px-2 py-1.5 text-muted">{t("None.")}</p>
      ) : (
        <table className="min-w-full border-collapse outline-none" tabIndex={0} onKeyDown={onKeyDown}>
          <thead>
            <tr>
              {section.columns.map((column, i) => (
                <th
                  key={i}
                  className={`border-r border-b border-line bg-panel-2 px-2 text-left font-semibold whitespace-nowrap text-muted ${
                    sticky ? `sticky z-10 ${bar ? "top-7" : "top-0"}` : ""
                  } ${column.kind === "number" ? "text-right" : column.kind === "flag" ? "text-center" : ""}`}
                  style={{ height: 22 }}
                >
                  {t(column.label)}
                </th>
              ))}
            </tr>
          </thead>
          <tbody ref={body}>
            {section.rows.map((row, r) => (
              <tr
                key={r}
                aria-selected={r === current}
                className={r === current ? "bg-selected" : "hover:bg-hover"}
                onMouseDown={() => onCurrent(r)}
                onDoubleClick={() => row.open && open(row.open)}
                onContextMenu={(e) => menu(e, row, r)}
                title={row.open ? t("Double-click to open {name}", { name: row.open.name }) : undefined}
              >
                {row.cells.map((cell, c) => (
                  <td
                    key={c}
                    className={`max-w-[36rem] truncate border-r border-b border-line/60 px-2 whitespace-nowrap ${
                      cellClass[section.columns[c]?.kind ?? "text"]
                    }`}
                    style={{ height: 22 }}
                  >
                    {cell}
                  </td>
                ))}
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </div>
  );
}

/** An object's script. Where the script creates the object over itself,
 *  it is how the object is changed: edited here, then compiled. */
function SqlText({ viewer, text, editable }: { viewer: Viewer; text: string; editable: boolean }) {
  const tab = viewer.active ?? "";
  const modified = viewer.drafts[tab] !== undefined;
  const host = useRef<HTMLDivElement>(null);
  const view = useRef<EditorView | null>(null);
  const current = () => view.current?.state.doc.toString() ?? text;
  // The newest of these, for the editor's keys, which are bound once
  const compile = useRef(() => {});
  compile.current = () => void compileTab(viewer, tab, current());

  useEffect(() => {
    if (!host.current) return;
    const { id } = viewer;
    const editing = editable
      ? [
          history(),
          indentOnInput(),
          bracketMatching(),
          closeBrackets(),
          highlightSelectionMatches(),
          EditorState.tabSize.of(2),
          Prec.highest(
            keymap.of(
              ["F9", "Mod-Enter"].map((key) => ({
                key,
                run: () => {
                  compile.current();
                  return true;
                },
              })),
            ),
          ),
          keymap.of([...closeBracketsKeymap, ...defaultKeymap, ...searchKeymap, ...historyKeymap, indentWithTab]),
          EditorView.updateListener.of((update) => {
            if (!update.docChanged) return;
            const doc = update.state.doc.toString();
            useViewers.getState().setDraft(id, tab, doc === text ? null : doc);
          }),
        ]
      : [EditorState.readOnly.of(true), EditorView.editable.of(false)];
    const v = new EditorView({
      parent: host.current,
      state: EditorState.create({
        // What was typed before the tab was left, or the object's own
        doc: useViewers.getState().viewers[id]?.drafts[tab] ?? text,
        extensions: [lineNumbers(), ...editing, firebirdSql, editorLook],
      }),
    });
    view.current = v;
    return () => {
      v.destroy();
      view.current = null;
    };
  }, [viewer.id, tab, text, editable]);

  const revert = () => {
    const v = view.current;
    if (v) v.dispatch({ changes: { from: 0, to: v.state.doc.length, insert: text } });
  };

  return (
    <div className="flex h-full flex-col">
      <div className="flex h-7 shrink-0 items-center gap-1 border-b border-line px-1.5">
        {editable && (
          <>
            <TextButton icon="run" label={t("Compile (F9)")} accent onClick={() => compile.current()} />
            <TextButton icon="rollback" label={t("Revert")} disabled={!modified} onClick={revert} />
            <span className="mx-1 h-4 w-px bg-line" />
          </>
        )}
        <TextButton icon="copy" label={t("Copy")} onClick={() => void navigator.clipboard.writeText(current())} />
        <TextButton icon="sql" label={t("Open in SQL Editor")} onClick={() => openEditor(viewer.registration, current())} />
        <span className="flex-1" />
        {modified && <span className="pr-1 text-muted">{t("Changed, not compiled")}</span>}
      </div>
      <div ref={host} className="min-h-0 flex-1 overflow-hidden" />
    </div>
  );
}

/** A text of the object's: its description */
function PlainText({ viewer, text, editable }: { viewer: Viewer; text: string; editable: boolean }) {
  const tab = viewer.active ?? "";
  const draft = viewer.drafts[tab];
  const value = draft ?? text;
  const setDraft = (next: string | null) => useViewers.getState().setDraft(viewer.id, tab, next);
  if (!editable) {
    return text.trim() ? (
      <pre className="scroll-thin h-full overflow-auto p-3 font-sans whitespace-pre-wrap select-text">{text}</pre>
    ) : (
      <Empty>{t("No description.")}</Empty>
    );
  }
  return (
    <div className="flex h-full flex-col">
      <div className="flex h-7 shrink-0 items-center gap-1 border-b border-line px-1.5">
        <TextButton icon="commit" label={t("Save…")} accent disabled={draft === undefined} onClick={() => void compileTab(viewer, tab, value)} />
        <TextButton icon="rollback" label={t("Revert")} disabled={draft === undefined} onClick={() => setDraft(null)} />
        <span className="flex-1" />
        {draft !== undefined && <span className="pr-1 text-muted">{t("Changed, not saved")}</span>}
      </div>
      <textarea
        value={value}
        onChange={(e) => setDraft(e.target.value === text ? null : e.target.value)}
        placeholder={t("No description.")}
        aria-label={t("Description")}
        spellCheck={false}
        className="scroll-thin min-h-0 flex-1 resize-none bg-panel p-3 outline-none placeholder:text-faint"
      />
    </div>
  );
}

export function TextButton(props: { icon: string; label: string; accent?: boolean; disabled?: boolean; onClick(): unknown }) {
  return (
    <button
      type="button"
      disabled={props.disabled}
      onClick={() => void props.onClick()}
      className="flex h-6 items-center gap-1.5 rounded px-2 text-fg hover:bg-hover disabled:text-faint disabled:hover:bg-transparent"
    >
      <Icon name={props.icon} size={13} className={props.accent && !props.disabled ? "text-ok" : ""} />
      {props.label}
    </button>
  );
}

/** The object's rows, through an editor tab of the viewer's own */
/** The kinds of column whose values the grid shows as they can be typed */
const typed = new Set(["integer", "decimal", "float", "text", "date", "time", "timestamp", "boolean"]);

/** Shows the script that makes the Data tab's changes the table's, to run */
async function applyEdits(viewer: Viewer) {
  const s = useStore.getState();
  try {
    const script = await api.dataChangesScript(viewer.registration, viewer.name, toChanges(viewer.edits));
    s.openDialog({
      type: "compile",
      registration: viewer.registration,
      title: t("Apply Changes: {name}", { name: viewer.name }),
      script,
      verb: t("Run and Commit"),
      done: () => {
        const viewers = useViewers.getState();
        viewers.setEdits(viewer.id, noEdits);
        viewers.refresh(viewer.id);
      },
    });
  } catch (e) {
    s.setNotice(errorText(e));
  }
}

function Data({ viewer, editing }: { viewer: Viewer; editing: RowEditing | null }) {
  const tab = useEditors((s) => (viewer.data ? s.tabs[viewer.data] : undefined));
  const [cursor, setCursor] = useState<GridCursor>({ row: 0, col: 0 });
  const resultId = tab?.outcome?.result?.id;
  const columns = tab?.outcome?.executed.columns;
  // The table's key and columns, as places in what the query brought
  const places = useMemo(() => {
    if (!editing || !columns) return null;
    const key = editing.key.map((name) => columns.findIndex((c) => c.name === name));
    if (key.some((k) => k < 0 || !typed.has(columns[k].kind))) return null;
    return { key, editable: columns.map((c) => editing.editable.includes(c.name) && typed.has(c.kind)) };
  }, [editing, columns]);
  const edit: ResultsEdit | undefined = places
    ? {
        ...places,
        edits: viewer.edits,
        onEdits: (edits: RowEdits) => useViewers.getState().setEdits(viewer.id, edits),
        onApply: () => void applyEdits(viewer),
        onRevert: () => useViewers.getState().setEdits(viewer.id, noEdits),
      }
    : undefined;
  useEffect(() => {
    setCursor({ row: 0, col: 0 });
  }, [resultId]);

  if (!tab || (tab.running && !tab.outcome)) return <Empty>{t("Loading…")}</Empty>;
  const last = tab.messages.at(-1);
  if (last?.level === "error" && !tab.outcome) {
    return <Empty>{<span className="whitespace-pre-wrap text-danger select-text">{last.text}</span>}</Empty>;
  }
  return (
    <div className="flex h-full flex-col">
      <div className="min-h-0 flex-1">
        <Results tab={tab} cursor={cursor} setCursor={setCursor} edit={edit} />
      </div>
      {!editing && viewer.kind === "table" && (
        <div className="shrink-0 truncate border-t border-line px-2 py-0.5 text-muted">
          {t("Read-only: the table has no primary key, nor a unique one without nulls, to tell its rows apart.")}
        </div>
      )}
      {last?.level === "error" && (
        <div className="shrink-0 truncate border-t border-line px-2 py-0.5 text-danger" role="alert" title={last.text}>
          {last.text}
        </div>
      )}
    </div>
  );
}
