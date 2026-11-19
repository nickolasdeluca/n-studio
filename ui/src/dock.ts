// The dock, for whoever opens panels in it: the toolbar, menus, shortcuts.
import type { DockviewApi } from "dockview-react";
import type { ObjectKind } from "./api";
import { pageOf, useEditors, type EditorFile, type EditorTab, type PageSeed } from "./editors";
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

/** Each editor's own title, "SQL Editor 1 · Sales": what its panel is
 *  called while the page in front has no file */
const titles = new Map<string, string>();

/** What an editor's panel is called: the file of the page in front, else its own title */
function shownTitle(tab: EditorTab): string {
  const page = pageOf(tab);
  return page?.file ? fileTitle({ file: page.file, changed: page.changed }) : (titles.get(tab.id) ?? tab.id);
}

/** Opens a new SQL editor on a connected registration, beside the others.
 *  It starts with `text`, or with `from.pages` (`from.front` in front). `from`
 *  also says where the text is from: a file, or an editor that comes back
 *  (`restored`) under the title it had. */
export function openEditor(
  registration: string,
  text?: string,
  from: { file?: EditorFile; title?: string; restored?: boolean; pages?: PageSeed[]; front?: number } = {},
): string | null {
  if (!dock || (!from.restored && !useStore.getState().servers[registration])) return null;
  const name = useStore.getState().registrations.find((r) => r.id === registration)?.name ?? "";
  const pages = from.pages ?? [{ text: text ?? "", file: from.file }];
  const tab = useEditors.getState().open(registration, false, pages, from.front);
  let title = from.title;
  if (!title) {
    count += 1;
    title = `${t("SQL Editor")} ${count}${name ? ` · ${name}` : ""}`;
  } else {
    // New editors are numbered after the ones that came back
    const n = Number(new RegExp(`^${t("SQL Editor")} (\\d+)`).exec(title)?.[1] ?? 0);
    count = Math.max(count, n);
  }
  titles.set(tab, title);
  dock.addPanel({
    id: tab,
    component: "sql",
    // Its close button asks first when a transaction is active
    tabComponent: "sql",
    title: shownTitle(useEditors.getState().tabs[tab]),
    params: { tab },
    position: beside(dock),
  });
  return tab;
}

/** An editor's own title, whatever file its page in front has */
export function editorTitle(id: string): string {
  return titles.get(id) ?? panelTitle(id);
}

let watching: (() => void) | undefined;

/** An editor's title is the file's name of the page in front once it has
 *  one, marked while the text is not what the file holds */
function retitle(state: ReturnType<typeof useEditors.getState>, before: ReturnType<typeof useEditors.getState>) {
  if (state.tabs === before.tabs) return;
  for (const tab of Object.values(state.tabs)) {
    const was = before.tabs[tab.id];
    if (!was || (was.page === tab.page && was.pages === tab.pages)) continue;
    const title = shownTitle(tab);
    const panel = dock?.getPanel(tab.id);
    if (panel && panel.title !== title) panel.api.setTitle(title);
  }
  for (const id of titles.keys()) if (!state.tabs[id]) titles.delete(id);
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
export function openAdminPanel(
  id: string,
  component: "admin" | "service" | "diagram" | "compare" | "builder" | "debug" | "executive",
  title: string,
) {
  if (!dock) return;
  const panel = dock.getPanel(id);
  if (panel) {
    panel.api.setActive();
    return;
  }
  // A Script Executive's close button asks first while it runs or has a
  // transaction active
  const tabComponent = component === "executive" ? "executive" : undefined;
  dock.addPanel({ id, component, tabComponent, title, params: { id }, position: beside(dock) });
}

export function setPanelTitle(id: string, title: string) {
  dock?.getPanel(id)?.api.setTitle(title);
}

export function closePanel(id: string) {
  dock?.getPanel(id)?.api.close();
}

/** A panel's title, without the mark of unsaved text */
export function panelTitle(id: string): string {
  const title = dock?.getPanel(id)?.title ?? id;
  return title.endsWith(CHANGED) ? title.slice(0, -CHANGED.length) : title;
}
