// An object's viewer: a row of tabs the backend names, each drawn by its
// shape. Nothing here knows what a Firebird table is made of; a row that
// stands for another object opens that object's viewer.
import { EditorState } from "@codemirror/state";
import { EditorView, lineNumbers } from "@codemirror/view";
import type { IDockviewPanelProps } from "dockview-react";
import { useEffect, useRef, useState } from "react";
import type { CellKind, GridRow, GridSection, ObjectRef, TabContent } from "../api";
import { extractMetadata } from "../actions";
import { openEditor, openObject } from "../dock";
import { useEditors } from "../editors";
import { editorLook, firebirdSql } from "../firebird";
import { t } from "../i18n";
import { Icon, kindColor } from "../icons";
import { useStore } from "../store";
import { useViewers, type Viewer } from "../viewers";
import { useContextMenu } from "./ContextMenu";
import type { GridCursor } from "./ResultGrid";
import { Empty, Results } from "./Results";

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
        {/* The explorer's heading is the plural; without the tree, the kind's own name */}
        {kind ? t(kind.label) : viewer.kind}
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
      return <Grids sections={content.sections} open={open} />;
    case "sql":
      return <SqlText text={content.text} registration={viewer.registration} />;
    case "text":
      return content.text.trim() ? (
        <pre className="scroll-thin h-full overflow-auto p-3 font-sans whitespace-pre-wrap select-text">{content.text}</pre>
      ) : (
        <Empty>{t("No description.")}</Empty>
      );
    case "data":
      return <Data viewer={viewer} />;
  }
}

const cellClass: Record<CellKind, string> = {
  text: "",
  number: "text-right tabular-nums",
  flag: "text-center text-ok",
  code: "font-mono text-[12px]",
};

/** A tab's grids, one under the other; a lone grid fills the tab */
function Grids({ sections, open }: { sections: GridSection[]; open(ref: ObjectRef): void }) {
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
          sticky={sections.length === 1}
        />
      ))}
    </div>
  );
}

function Section(props: {
  section: GridSection;
  current: number;
  onCurrent(row: number): void;
  open(ref: ObjectRef): void;
  /** Its header stays in view: only when it is the tab's one grid */
  sticky: boolean;
}) {
  const { section, current, onCurrent, open, sticky } = props;
  const showMenu = useContextMenu((m) => m.show);
  const body = useRef<HTMLTableSectionElement>(null);

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
    ]);
  };

  return (
    <div className={section.title ? "mb-3" : ""}>
      {section.title && (
        <div className="flex h-7 items-center gap-2 border-b border-line bg-panel-2 px-2 font-semibold">
          {t(section.title)}
          <span className="font-normal text-faint tabular-nums">{section.rows.length.toLocaleString()}</span>
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
                    sticky ? "sticky top-0 z-10" : ""
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

/** DDL, as the editor would show it, not to be edited here */
function SqlText({ text, registration }: { text: string; registration: string }) {
  const host = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!host.current) return;
    const view = new EditorView({
      parent: host.current,
      state: EditorState.create({
        doc: text,
        extensions: [lineNumbers(), EditorState.readOnly.of(true), EditorView.editable.of(false), firebirdSql, editorLook],
      }),
    });
    return () => view.destroy();
  }, [text]);
  return (
    <div className="flex h-full flex-col">
      <div className="flex h-7 shrink-0 items-center gap-1 border-b border-line px-1.5">
        <TextButton icon="copy" label={t("Copy")} onClick={() => void navigator.clipboard.writeText(text)} />
        <TextButton icon="sql" label={t("Open in SQL Editor")} onClick={() => openEditor(registration, text)} />
      </div>
      <div ref={host} className="min-h-0 flex-1 overflow-hidden" />
    </div>
  );
}

function TextButton(props: { icon: string; label: string; onClick(): unknown }) {
  return (
    <button
      onClick={() => void props.onClick()}
      className="flex h-6 items-center gap-1.5 rounded px-2 text-fg hover:bg-hover"
    >
      <Icon name={props.icon} size={13} />
      {props.label}
    </button>
  );
}

/** The object's rows, through an editor tab of the viewer's own */
function Data({ viewer }: { viewer: Viewer }) {
  const tab = useEditors((s) => (viewer.data ? s.tabs[viewer.data] : undefined));
  const [cursor, setCursor] = useState<GridCursor>({ row: 0, col: 0 });
  const resultId = tab?.outcome?.result?.id;
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
        <Results tab={tab} cursor={cursor} setCursor={setCursor} />
      </div>
      {last?.level === "error" && (
        <div className="shrink-0 truncate border-t border-line px-2 py-0.5 text-danger" role="alert" title={last.text}>
          {last.text}
        </div>
      )}
    </div>
  );
}
