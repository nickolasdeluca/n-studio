import { listen } from "@tauri-apps/api/event";
import {
  DockviewDefaultTab,
  DockviewReact,
  type DockviewApi,
  type DockviewGroupPanel,
  type DockviewReadyEvent,
  type DockviewTheme,
  type IDockviewPanelHeaderProps,
  type IDockviewPanelProps,
} from "dockview-react";
import { getVersion } from "@tauri-apps/api/app";
import { useEffect, useRef, useState, type ReactNode } from "react";
import { forgetService, useAdmin } from "../admin";
import { serverSummary } from "../api";
import { isDocument, openEditor, setDock } from "../dock";
import { useBuilders } from "../builder";
import { forgetComparison } from "../compare";
import { forgetDebug } from "../debug";
import { useDiagrams } from "../diagram";
import { useEditors } from "../editors";
import { t } from "../i18n";
import { guardQuit, request } from "../pending";
import { checkForUpdate } from "../update";
import { openScripts } from "../scripts";
import { restoreSession, sessionChanged } from "../session";
import { Icon } from "../icons";
import { useStore } from "../store";
import { runKey, useKeys, withKey } from "../keys";
import { zoom } from "../fonts";
import { hasChanges, useViewers } from "../viewers";
import { ContextMenuHost, useContextMenu, type MenuItem } from "./ContextMenu";
import { AdminView, ServiceOutput } from "./AdminPanels";
import { AgentPanel } from "./AgentPanel";
import { AGENTS, agentName, useAgent } from "../agent";
import { CompareView } from "./CompareView";
import { DebugView } from "./DebugView";
import { DiagramView } from "./DiagramView";
import { QueryBuilder } from "./QueryBuilder";
import { Dialogs } from "./Dialogs";
import { Explorer } from "./Explorer";
import { ObjectList } from "./ObjectList";
import { ObjectViewer } from "./ObjectViewer";
import { forgetEditor, SqlEditor } from "./SqlEditor";
import { forgetExecutive, ScriptExecutive } from "./ScriptExecutive";
import { closeExecutive, openExecutive } from "../executive";

const theme: DockviewTheme = { name: "n", className: "dockview-theme-n" };
const LAYOUT = "n-studio.layout.v1";

const components: Record<string, React.FunctionComponent<IDockviewPanelProps>> = {
  explorer: () => <Explorer />,
  objects: () => <ObjectList />,
  start: () => <Start />,
  agent: () => <AgentPanel />,
  sql: (props) => <SqlEditor {...(props as IDockviewPanelProps<{ tab: string }>)} />,
  object: (props) => <ObjectViewer {...(props as IDockviewPanelProps<{ viewer: string }>)} />,
  admin: (props) => <AdminView {...(props as IDockviewPanelProps<{ id: string }>)} />,
  service: (props) => <ServiceOutput {...(props as IDockviewPanelProps<{ id: string }>)} />,
  diagram: (props) => <DiagramView {...(props as IDockviewPanelProps<{ id: string }>)} />,
  compare: (props) => <CompareView {...(props as IDockviewPanelProps<{ id: string }>)} />,
  builder: (props) => <QueryBuilder {...(props as IDockviewPanelProps<{ id: string }>)} />,
  debug: (props) => <DebugView {...(props as IDockviewPanelProps<{ id: string }>)} />,
  executive: (props) => <ScriptExecutive {...(props as IDockviewPanelProps<{ id: string }>)} />,
};

const tabComponents: Record<string, React.FunctionComponent<IDockviewPanelHeaderProps>> = {
  object: (props) => (
    <DockviewDefaultTab
      {...props}
      closeActionOverride={() => {
        // Text typed and not compiled is asked about before it goes
        const viewer = useViewers.getState().viewers[props.api.id];
        if (viewer && hasChanges(viewer)) useStore.getState().openDialog({ type: "discard", viewers: [props.api.id] });
        else props.api.close();
      }}
    />
  ),
  sql: (props) => (
    <DockviewDefaultTab {...props} closeActionOverride={() => request({ kind: "close", tab: props.api.id })} />
  ),
  executive: (props) => (
    <DockviewDefaultTab {...props} closeActionOverride={() => request({ kind: "close", tab: props.api.id })} />
  ),
};

/** The panels the View menu can bring back */
const panels: { id: string; title: string; right?: boolean }[] = [
  { id: "explorer", title: "Database Explorer" },
  { id: "objects", title: "Objects" },
  { id: "agent", title: "AI Agent", right: true },
];

export function Shell() {
  const dock = useRef<DockviewApi | null>(null);

  useEffect(() => {
    void useStore.getState().load();
    void guardQuit();
  }, []);

  // The same choices from the menu bar's View menu, where there is one
  useEffect(() => {
    const heard = listen<string>("view-menu", (e) => {
      const api = dock.current;
      if (!api) return;
      if (e.payload === "reset") defaultLayout(api);
      else showPanel(api, e.payload);
    });
    return () => void heard.then((stop) => stop());
  }, []);

  // The app's own keys, wherever the keyboard is: F12, as in IBExpert, an
  // SQL editor on the selected database, Ctrl+F12 the Script Executive on it
  // or on none, Ctrl+O (Cmd+O on a Mac, where Ctrl+O splits a line) a script
  // into an editor. A key a place took for its own is not one of these.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.defaultPrevented) return;
      const s = useStore.getState();
      const id = s.selection?.id;
      runKey("app", e, {
        "app.newEditor": () => (id ? void openEditor(id) : false),
        "app.openScript": () => (id && s.servers[id] ? void openScripts(id) : false),
        "app.executive": () => void openExecutive(id ?? null),
        "app.preferences": () => s.openDialog({ type: "preferences" }),
        "app.zoomIn": () => zoom(1),
        "app.zoomOut": () => zoom(-1),
        "app.zoomReset": () => zoom(0),
      });
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  const onReady = (e: DockviewReadyEvent) => {
    dock.current = e.api;
    setDock(e.api);
    if (!restore(e.api)) defaultLayout(e.api);
    // The panel is called after the agent chosen, which a stopped panel can
    // change
    e.api.getPanel("agent")?.api.setTitle(agentName());
    useAgent.subscribe((now, before) => {
      if (now.kind !== before.kind) e.api.getPanel("agent")?.api.setTitle(agentName());
    });
    void restoreSession();
    // An editor's tab closing ends its transaction
    e.api.onDidRemovePanel((panel) => {
      // The agent goes with its panel
      if (panel.id === "agent") {
        void useAgent.getState().stop();
        agentClosed(e.api);
      }
      if (panel.id.startsWith("obj-")) useViewers.getState().close(panel.id);
      // A service's tab closing stops listening to it
      if (panel.id.startsWith("adm-svc-")) forgetService(panel.id);
      else if (panel.id.startsWith("adm-dgm-")) useDiagrams.getState().close(panel.id);
      else if (panel.id.startsWith("adm-cmp-")) forgetComparison(panel.id);
      else if (panel.id.startsWith("adm-qry-")) useBuilders.getState().close(panel.id);
      else if (panel.id.startsWith("adm-dbg-")) forgetDebug(panel.id);
      else if (panel.id.startsWith("adm-exe-")) {
        closeExecutive(panel.id);
        forgetExecutive(panel.id);
      }
      else if (panel.id.startsWith("adm-")) useAdmin.getState().close(panel.id);
      if (!panel.id.startsWith("sql-")) return;
      forgetEditor(panel.id);
      void useEditors.getState().close(panel.id);
    });
    e.api.onDidLayoutChange(() => {
      // Kept while the agent's panel is open, for when it closes, and its
      // own width for when it opens again
      const agent = e.api.getPanel("agent");
      if (agent) {
        widthsWithAgent = widthsOf(e.api);
        rememberAgentWidth(agent.group.api.width);
      }
      try {
        localStorage.setItem(LAYOUT, JSON.stringify(e.api.toJSON()));
      } catch {
        // The layout is a convenience; without storage it resets next time
      }
      // Which editors there are, their order and the one in front
      sessionChanged();
    });
  };

  return (
    <div className="flex h-full flex-col">
      <Toolbar dock={dock} />
      <div className="min-h-0 flex-1">
        <DockviewReact components={components} tabComponents={tabComponents} onReady={onReady} theme={theme} />
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
    // Editors and viewers do not outlive the app: their transactions and
    // connections are gone
    for (const panel of api.panels.filter((p) => isDocument(p.id))) panel.api.close();
    if (!api.getPanel("start")) api.addPanel({ id: "start", component: "start", title: t("Start") });
    // The agent's panel came back: the others go back to these when it closes
    if (api.getPanel("agent")) widthsWithAgent = widthsOf(api);
    return api.panels.length > 0;
  } catch {
    return false;
  }
}

const AGENT_WIDTH = "n-studio.agent.width";
/** The narrowest the agent's panel opens, and what it leaves the center */
const AGENT_MIN = 320;
const CENTER_MIN = 240;

/** The agent panel's width when it opens: the one it last had, else a
 *  quarter of the screen */
function agentWidth() {
  try {
    const saved = Number(localStorage.getItem(AGENT_WIDTH));
    if (saved > 0) return saved;
  } catch {
    // Without storage it opens at the default
  }
  return Math.round(window.screen.width * 0.25);
}

function rememberAgentWidth(width: number) {
  if (width <= 0) return;
  try {
    localStorage.setItem(AGENT_WIDTH, String(Math.round(width)));
  } catch {
    // The width is a convenience
  }
}

/** The dock's groups' widths, as they last were while the agent's panel
 *  was open: what the others go back to when it closes */
let widthsWithAgent: Map<string, number> | null = null;

const widthsOf = (api: DockviewApi) => new Map(api.groups.map((g) => [g.id, g.api.width]));

/** The group of the center's documents */
function centerGroup(api: DockviewApi) {
  const panel =
    api.panels.find((p) => isDocument(p.id) && p.group.api.isActive) ??
    api.panels.find((p) => isDocument(p.id)) ??
    api.getPanel("start");
  return panel?.group ?? null;
}

/** Gives every group but `free` the width it had, `free` taking what is
 *  left. The dock hands a group's change of width to its last neighbour, so
 *  each group is held at its width once given it, until only `free` can
 *  move, and let go after. */
function keepWidths(api: DockviewApi, widths: Map<string, number>, free: string) {
  const held: { group: DockviewGroupPanel; minimumWidth: number; maximumWidth: number }[] = [];
  for (const group of api.groups) {
    const width = widths.get(group.id);
    if (group.id === free || width === undefined) continue;
    held.push({ group, minimumWidth: group.minimumWidth, maximumWidth: group.maximumWidth });
    group.api.setSize({ width });
    group.api.setConstraints({ minimumWidth: group.api.width, maximumWidth: group.api.width });
  }
  for (const { group, minimumWidth, maximumWidth } of held) group.api.setConstraints({ minimumWidth, maximumWidth });
}

/** Opens a panel to the right of the center's documents, in their room only:
 *  the other panels keep their widths */
function openBesideCenter(api: DockviewApi, id: string, title: string) {
  const center = centerGroup(api);
  if (!center) {
    api.addPanel({ id, component: id, title, position: { direction: "right" }, initialWidth: 560 });
    return;
  }
  const widths = widthsOf(api);
  const room = center.api.width;
  api.addPanel({ id, component: id, title, position: { referenceGroup: center, direction: "right" } });
  const added = api.getPanel(id);
  if (!added) return;
  // The width it last had, else a quarter of the screen; the center keeps
  // room for its tabs
  widths.set(added.group.id, Math.max(AGENT_MIN, Math.min(agentWidth(), room - CENTER_MIN)));
  keepWidths(api, widths, center.id);
  widthsWithAgent = widthsOf(api);
}

/** The agent's panel closed: the others get back the widths they had, and
 *  the center what it gave. Its group goes after the panel, sharing its room
 *  out evenly, so this waits for that. */
function agentClosed(api: DockviewApi) {
  const widths = widthsWithAgent;
  widthsWithAgent = null;
  if (!widths) return;
  setTimeout(() => {
    const center = centerGroup(api);
    if (center && !api.getPanel("agent")) keepWidths(api, widths, center.id);
  }, 0);
}

/** Brings a panel of the View menu to the front, or back if it was closed */
function showPanel(api: DockviewApi, id: string) {
  const known = panels.find((p) => p.id === id);
  if (!known) return;
  const panel = api.getPanel(id);
  if (panel) panel.api.setActive();
  else if (id === "agent") openBesideCenter(api, id, agentName());
  else if (known.right) openBesideCenter(api, id, t(known.title));
  else api.addPanel({ id, component: id, title: t(known.title), position: { direction: "left" }, initialWidth: 300 });
}

function Toolbar({ dock }: { dock: React.RefObject<DockviewApi | null> }) {
  useKeys();
  const agent = AGENTS[useAgent((s) => s.kind)].name;
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
      onSelect: () => showPanel(api, p.id),
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
      <ToolButton icon="import" label={t("Import from IBExpert…")} onClick={() => s.openDialog({ type: "ibexpert" })} />
      <Separator />
      <ToolButton icon="plug" label={t("Connect")} disabled={!id || connected || connecting} onClick={() => id && void s.connect(id)} />
      <ToolButton icon="unplug" label={t("Disconnect")} disabled={!connected} onClick={() => id && void s.disconnect(id)} />
      <ToolButton icon="refresh" label={t("Refresh")} disabled={!connected} onClick={() => id && void s.refresh(id)} />
      <Separator />
      <ToolButton icon="sql" label={withKey(t("New SQL Editor"), "app.newEditor")} disabled={!connected} onClick={() => id && openEditor(id)} />
      <ToolButton icon="folder" label={withKey(t("Open Script…"), "app.openScript")} disabled={!connected} onClick={() => id && openScripts(id)} />
      <ToolButton icon="run" label={withKey(t("Script Executive"), "app.executive")} onClick={() => openExecutive(id ?? null)} />
      <ToolButton icon="agent" label={t("AI Agent ({agent})", { agent })} onClick={() => dock.current && showPanel(dock.current, "agent")} />
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
      <ToolButton
        icon="settings"
        label={withKey(t("Preferences…"), "app.preferences")}
        onClick={() => s.openDialog({ type: "preferences" })}
      />
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
  const busy = useStore((s) => s.busy);
  // The bundle's version, so a report can say which build it is about
  const [version, setVersion] = useState("");
  const update = useStore((s) => s.update);
  const updating = useStore((s) => s.updating);
  // What a check asked for by hand answered, shown for a moment
  const [checked, setChecked] = useState("");
  useEffect(() => {
    getVersion().then(setVersion, () => {});
    void checkForUpdate();
  }, []);

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
      {busy && (
        <span className="flex min-w-0 items-center gap-1.5" role="status">
          <span className="size-1.5 animate-pulse rounded-full bg-accent" />
          <span className="truncate">{busy}</span>
        </span>
      )}
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
      <span className={`flex shrink-0 items-center gap-2 ${notice ? "" : "ml-auto"}`}>
        {updating !== null ? (
          <span className="text-fg tabular-nums" role="status">
            {updating === true
              ? t("Downloading {version}…", { version: update?.version ?? "" })
              : t("Downloading {version}… {done}%", { version: update?.version ?? "", done: Math.round(updating * 100) })}
          </span>
        ) : (
          update && (
            <span className="flex items-center gap-1.5" role="status">
              <span className="size-1.5 rounded-full bg-accent" />
              <span className="text-fg select-text" title={update.page}>
                {update.installable
                  ? t("Version {version} is available", { version: update.version })
                  : t("Version {version} is available at {page}", { version: update.version, page: update.page })}
              </span>
              {update.installable && (
                <button className="rounded px-1.5 text-accent hover:bg-hover" onClick={() => request({ kind: "update" })}>
                  {t("Update and Restart")}
                </button>
              )}
              <button aria-label={t("Later")} title={t("Later")} className="px-1 hover:text-fg" onClick={() => useStore.setState({ update: null })}>
                ×
              </button>
            </span>
          )
        )}
        {version && (
          <button
            className="text-faint hover:text-fg"
            title={t("Check for updates")}
            onClick={async () => {
              setChecked(t("Checking…"));
              const found = await checkForUpdate(true);
              setChecked(found ? "" : t("Up to date"));
              setTimeout(() => setChecked(""), 4000);
            }}
          >
            {checked || `v${version}`}
          </button>
        )}
      </span>
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
            : t("Double-click a database in the explorer to connect, then an object to open it, or press F12 for an SQL editor.")}
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
