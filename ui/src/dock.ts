// The dock, for whoever opens panels in it: the toolbar, menus, shortcuts.
import type { DockviewApi } from "dockview-react";
import { useEditors } from "./editors";
import { t } from "./i18n";
import { useStore } from "./store";

let dock: DockviewApi | null = null;
let count = 0;

export function setDock(api: DockviewApi) {
  dock = api;
}

/** Opens a new SQL editor on a connected registration, beside the others */
export function openEditor(registration: string) {
  if (!dock || !useStore.getState().servers[registration]) return;
  const name = useStore.getState().registrations.find((r) => r.id === registration)?.name ?? "";
  const tab = useEditors.getState().open(registration);
  count += 1;
  // In the center, with the other editors or the start page
  const beside = dock.panels.find((p) => p.id.startsWith("sql-")) ?? dock.getPanel("start");
  dock.addPanel({
    id: tab,
    component: "sql",
    // Its close button asks first when a transaction is active
    tabComponent: "sql",
    title: `${t("SQL Editor")} ${count}${name ? ` · ${name}` : ""}`,
    params: { tab },
    position: beside ? { referencePanel: beside.id } : { direction: "right" },
  });
}

export function closePanel(id: string) {
  dock?.getPanel(id)?.api.close();
}

export function panelTitle(id: string): string {
  return dock?.getPanel(id)?.title ?? id;
}
