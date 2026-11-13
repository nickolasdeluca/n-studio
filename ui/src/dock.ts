// The dock, for whoever opens panels in it: the toolbar, menus, shortcuts.
import type { DockviewApi } from "dockview-react";
import type { ObjectKind } from "./api";
import { drafts, useEditors } from "./editors";
import { t } from "./i18n";
import { useStore } from "./store";
import { useViewers } from "./viewers";

let dock: DockviewApi | null = null;
let count = 0;

export function setDock(api: DockviewApi) {
  dock = api;
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

/** Opens a new SQL editor on a connected registration, beside the others;
 *  `text` is what it starts with */
export function openEditor(registration: string, text?: string) {
  if (!dock || !useStore.getState().servers[registration]) return;
  const name = useStore.getState().registrations.find((r) => r.id === registration)?.name ?? "";
  const tab = useEditors.getState().open(registration);
  if (text) drafts.set(tab, text);
  count += 1;
  dock.addPanel({
    id: tab,
    component: "sql",
    // Its close button asks first when a transaction is active
    tabComponent: "sql",
    title: `${t("SQL Editor")} ${count}${name ? ` · ${name}` : ""}`,
    params: { tab },
    position: beside(dock),
  });
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

export function panelTitle(id: string): string {
  return dock?.getPanel(id)?.title ?? id;
}
