// The dock, for whoever opens panels in it: the toolbar, menus, shortcuts.
import type { DockviewApi } from "dockview-react";
import type { ObjectKind } from "./api";
import { drafts, useEditors, type EditorFile } from "./editors";
import { t } from "./i18n";
import { useStore } from "./store";
import { useViewers } from "./viewers";

let dock: DockviewApi | null = null;
let count = 0;

export function setDock(api: DockviewApi) {
  dock = api;
  watching ??= useEditors.subscribe(retitle);
}

/** Editors, object viewers and the administration tools: what the center
 *  of the window holds */
export const isDocument = (panel: string) => ["sql-", "obj-", "adm-"].some((prefix) => panel.startsWith(prefix));

/** Where a new document goes: with the others, or the start page */
function beside(api: DockviewApi) {
  const active = api.activePanel;
  const panel = (active && isDocument(active.id) ? active : api.panels.find((p) => isDocument(p.id))) ?? api.getPanel("start");
  return panel ? { referencePanel: panel.id } : ({ direction: "right" } as const);
}

/** After the title of an editor whose text is not what its file holds */
const CHANGED = " •";

/** A path's last part */
export const baseName = (path: string) => path.split(/[\\/]/).pop() || path;

const fileTitle = (file: EditorFile | { file: string; changed: boolean }) => baseName(file.file) + (file.changed ? CHANGED : "");

/** Opens a new SQL editor on a connected registration, beside the others;
 *  `text` is what it starts with. `from` says where the text is from: a
 *  file, or the last run of the app (`restored`), whose editors come back
 *  under the titles they had and before anything is connected. */
export function openEditor(
  registration: string,
  text?: string,
  from: { file?: EditorFile; title?: string; restored?: boolean } = {},
): string | null {
  if (!dock || (!from.restored && !useStore.getState().servers[registration])) return null;
  const name = useStore.getState().registrations.find((r) => r.id === registration)?.name ?? "";
  const tab = useEditors.getState().open(registration, false, from.file);
  if (text) drafts.set(tab, text);
  let title = from.file ? fileTitle(from.file) : from.title;
  if (!title) {
    count += 1;
    title = `${t("SQL Editor")} ${count}${name ? ` · ${name}` : ""}`;
  } else {
    // New editors are numbered after the ones that came back
    const n = Number(new RegExp(`^${t("SQL Editor")} (\\d+)`).exec(title)?.[1] ?? 0);
    count = Math.max(count, n);
  }
  dock.addPanel({
    id: tab,
    component: "sql",
    // Its close button asks first when a transaction is active
    tabComponent: "sql",
    title,
    params: { tab },
    position: beside(dock),
  });
  return tab;
}

let watching: (() => void) | undefined;

/** An editor's title is its file's name once it has one, marked while the
 *  text is not what the file holds */
function retitle(state: ReturnType<typeof useEditors.getState>, before: ReturnType<typeof useEditors.getState>) {
  if (state.tabs === before.tabs) return;
  for (const tab of Object.values(state.tabs)) {
    const was = before.tabs[tab.id];
    if (tab.file && was && (tab.file !== was.file || tab.changed !== was.changed)) {
      dock?.getPanel(tab.id)?.api.setTitle(fileTitle({ file: tab.file, changed: tab.changed }));
    }
  }
}

/** The SQL editors' panels in the order the dock has them, and the one in
 *  front, if it is one of them */
export function editorPanels(): { ids: string[]; active: string | null } {
  const ids = (dock?.panels ?? []).map((p) => p.id).filter((id) => id.startsWith("sql-"));
  const active = dock?.activePanel?.id ?? null;
  return { ids, active: active && ids.includes(active) ? active : null };
}

export function activatePanel(id: string) {
  dock?.getPanel(id)?.api.setActive();
}

/** Shows an object's viewer: the one it has, or a new one */
export function openObject(registration: string, kind: ObjectKind, name: string) {
  if (!dock || !useStore.getState().servers[registration]) return;
  const id = useViewers.getState().open(registration, kind, name);
  const panel = dock.getPanel(id);
  if (panel) {
    panel.api.setActive();
    return;
  }
  dock.addPanel({
    id,
    component: "object",
    // Its close button asks first when text was changed and not compiled
    tabComponent: "object",
    title: name,
    params: { viewer: id },
    position: beside(dock),
  });
}

/** Shows a tool's panel: the one with that id, or a new one */
export function openAdminPanel(id: string, component: "admin" | "service" | "diagram" | "compare" | "builder" | "debug", title: string) {
  if (!dock) return;
  const panel = dock.getPanel(id);
  if (panel) {
    panel.api.setActive();
    return;
  }
  dock.addPanel({ id, component, title, params: { id }, position: beside(dock) });
}

export function closePanel(id: string) {
  dock?.getPanel(id)?.api.close();
}

/** A panel's title, without the mark of unsaved text */
export function panelTitle(id: string): string {
  const title = dock?.getPanel(id)?.title ?? id;
  return title.endsWith(CHANGED) ? title.slice(0, -CHANGED.length) : title;
}
