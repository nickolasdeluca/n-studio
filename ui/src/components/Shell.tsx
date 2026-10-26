import {
  DockviewReact,
  type DockviewApi,
  type DockviewReadyEvent,
  type DockviewTheme,
  type IDockviewPanelProps,
} from "dockview-react";
import { useEffect, useRef, type ReactNode } from "react";
import { serverSummary } from "../api";
import { openEditor, setDock } from "../dock";
import { useEditors } from "../editors";
import { t } from "../i18n";
import { Icon } from "../icons";
import { useStore } from "../store";
import { ContextMenuHost, useContextMenu, type MenuItem } from "./ContextMenu";
import { Dialogs } from "./Dialogs";
import { Explorer } from "./Explorer";
import { ObjectList } from "./ObjectList";
import { forgetEditor, SqlEditor } from "./SqlEditor";

const theme: DockviewTheme = { name: "n", className: "dockview-theme-n" };
const LAYOUT = "n-studio.layout.v1";

const components: Record<string, React.FunctionComponent<IDockviewPanelProps>> = {
  explorer: () => <Explorer />,
  objects: () => <ObjectList />,
  start: () => <Start />,
  sql: (props) => <SqlEditor {...(props as IDockviewPanelProps<{ tab: string }>)} />,
};

/** The panels the View menu can bring back */
const panels = [
  { id: "explorer", title: "Database Explorer" },
  { id: "objects", title: "Objects" },
];

export function Shell() {
  const dock = useRef<DockviewApi | null>(null);

  useEffect(() => {
    void useStore.getState().load();
  }, []);

  // F12, as in IBExpert: an SQL editor on the selected database
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "F12") return;
      const id = useStore.getState().selection?.id;
      if (!id) return;
      e.preventDefault();
      openEditor(id);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  const onReady = (e: DockviewReadyEvent) => {
    dock.current = e.api;
    setDock(e.api);
    if (!restore(e.api)) defaultLayout(e.api);
    // An editor's tab closing ends its transaction
    e.api.onDidRemovePanel((panel) => {
      if (!panel.id.startsWith("sql-")) return;
      forgetEditor(panel.id);
      void useEditors.getState().close(panel.id);
    });
    e.api.onDidLayoutChange(() => {
      try {
        localStorage.setItem(LAYOUT, JSON.stringify(e.api.toJSON()));
      } catch {
        // The layout is a convenience; without storage it resets next time
      }
    });
  };

  return (
    <div className="flex h-full flex-col">
      <Toolbar dock={dock} />
      <div className="min-h-0 flex-1">
        <DockviewReact components={components} onReady={onReady} theme={theme} />
      </div>
      <StatusBar />
      <Dialogs />
      <ContextMenuHost />
    </div>
  );
}

function defaultLayout(api: DockviewApi) {
  api.clear();
  api.addPanel({ id: "start", component: "start", title: t("Start") });
  api.addPanel({
    id: "explorer",
    component: "explorer",
    title: t("Database Explorer"),
    position: { direction: "left" },
    initialWidth: 300,
  });
  api.addPanel({
    id: "objects",
    component: "objects",
    title: t("Objects"),
    position: { referencePanel: "explorer", direction: "below" },
    initialHeight: 280,
  });
}

function restore(api: DockviewApi): boolean {
  try {
    const saved = localStorage.getItem(LAYOUT);
    if (!saved) return false;
    api.fromJSON(JSON.parse(saved));
    // Editors do not outlive the app: their transactions are gone
    for (const panel of api.panels.filter((p) => p.id.startsWith("sql-"))) panel.api.close();
    if (!api.getPanel("start")) api.addPanel({ id: "start", component: "start", title: t("Start") });
    return api.panels.length > 0;
  } catch {
    return false;
  }
}

function Toolbar({ dock }: { dock: React.RefObject<DockviewApi | null> }) {
  const selection = useStore((s) => s.selection);
  const connected = useStore((s) => (selection ? !!s.servers[selection.id] : false));
  const connecting = useStore((s) => (selection ? !!s.connecting[selection.id] : false));
  const showMenu = useContextMenu((m) => m.show);
  const s = useStore.getState();
  const id = selection?.id;

  const viewMenu = (e: React.MouseEvent) => {
    const api = dock.current;
    if (!api) return;
    const rect = e.currentTarget.getBoundingClientRect();
    const items: MenuItem[] = panels.map((p) => ({
      label: t(p.title),
      icon: "panel",
      onSelect: () => {
        const panel = api.getPanel(p.id);
        if (panel) panel.api.setActive();
        else
          api.addPanel({
            id: p.id,
            component: p.id,
            title: t(p.title),
            position: { direction: "left" },
            initialWidth: 300,
          });
      },
    }));
    showMenu(rect.left, rect.bottom + 2, [
      ...items,
      "separator",
      { label: t("Reset Layout"), icon: "refresh", onSelect: () => defaultLayout(api) },
    ]);
  };

  return (
    <div className="flex h-9 shrink-0 items-center gap-0.5 border-b border-line bg-panel-2 px-2">
      <span className="mr-2 grid size-6 place-items-center rounded bg-accent text-[13px] font-bold text-accent-fg" title="N Studio">
        N
      </span>
      <ToolButton icon="plus" label={t("Register Database…")} onClick={() => s.openDialog({ type: "registration", registration: null })} />
      <Separator />
      <ToolButton icon="plug" label={t("Connect")} disabled={!id || connected || connecting} onClick={() => id && void s.connect(id)} />
      <ToolButton icon="unplug" label={t("Disconnect")} disabled={!connected} onClick={() => id && void s.disconnect(id)} />
      <ToolButton icon="refresh" label={t("Refresh")} disabled={!connected} onClick={() => id && void s.refresh(id)} />
      <Separator />
      <ToolButton icon="sql" label={t("New SQL Editor (F12)")} disabled={!connected} onClick={() => id && openEditor(id)} />
      <Separator />
      <ToolButton
        icon="edit"
        label={t("Edit Registration…")}
        disabled={!id}
        onClick={() => {
          const r = useStore.getState().registrations.find((r) => r.id === id);
          if (r) s.openDialog({ type: "registration", registration: r });
        }}
      />
      <span className="flex-1" />
      <button className="flex h-7 items-center gap-1.5 rounded px-2 hover:bg-hover" onClick={viewMenu}>
        <Icon name="panel" />
        {t("View")}
      </button>
    </div>
  );
}

function ToolButton(props: {
  icon: string;
  label: string;
  disabled?: boolean;
  onClick: () => unknown;
}) {
  return (
    <button
      title={props.label}
      aria-label={props.label}
      disabled={props.disabled}
      onClick={() => void props.onClick()}
      className="grid size-7 place-items-center rounded text-fg hover:bg-hover disabled:text-faint disabled:hover:bg-transparent"
    >
      <Icon name={props.icon} size={15} />
    </button>
  );
}

function Separator() {
  return <span className="mx-1 h-4 w-px bg-line" />;
}

function StatusBar() {
  const selection = useStore((s) => s.selection);
  const registration = useStore((s) => s.registrations.find((r) => r.id === selection?.id));
  const server = useStore((s) => (selection ? s.servers[selection.id] : undefined));
  const notice = useStore((s) => s.notice);

  let left: ReactNode = t("No database selected");
  if (registration) {
    left = (
      <>
        <span className={`size-1.5 rounded-full ${server ? "bg-ok" : "bg-faint"}`} />
        <span className="text-fg">{registration.name}</span>
        <span>{server ? serverSummary(server) : t("Not connected")}</span>
      </>
    );
  }

  return (
    <div className="flex h-6 shrink-0 items-center gap-2 border-t border-line bg-panel-2 px-2 text-[11.5px] text-muted">
      {left}
      {notice && (
        <span className="ml-auto flex min-w-0 items-center gap-1.5 text-danger" role="alert">
          <span className="truncate" title={notice}>
            {notice}
          </span>
          <button
            aria-label={t("Dismiss")}
            className="px-1 hover:text-fg"
            onClick={() => useStore.getState().setNotice(null)}
          >
            ×
          </button>
        </span>
      )}
    </div>
  );
}

/** The center area before any editor opens */
function Start() {
  const count = useStore((s) => s.registrations.length);
  return (
    <div className="grid h-full place-items-center bg-bg">
      <div className="flex max-w-sm flex-col items-center gap-3 text-center">
        <span className="grid size-14 place-items-center rounded-xl bg-accent text-3xl font-bold text-accent-fg">N</span>
        <h1 className="text-lg font-semibold">N Studio</h1>
        <p className="text-muted">{t("Any database, One IDE.")}</p>
        <p className="text-muted">
          {count === 0
            ? t("Register a database to get started.")
            : t("Double-click a database in the explorer to connect, then press F12 for an SQL editor.")}
        </p>
        {count === 0 && (
          <button
            className="mt-1 h-7 rounded border border-accent bg-accent px-3 text-accent-fg hover:brightness-110"
            onClick={() => useStore.getState().openDialog({ type: "registration", registration: null })}
          >
            {t("Register Database…")}
          </button>
        )}
      </div>
    </div>
  );
}
