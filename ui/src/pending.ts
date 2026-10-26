// Editors with an active transaction are never closed behind the user's
// back: closing one, disconnecting its database or quitting asks first
// whether to commit or roll back.
import { getCurrentWindow } from "@tauri-apps/api/window";
import { closePanel, panelTitle } from "./dock";
import { useEditors } from "./editors";
import { useStore } from "./store";

export type PendingAction = { kind: "close"; tab: string } | { kind: "disconnect"; id: string } | { kind: "quit" };

/** Editors with an active transaction, optionally only some */
export function activeTabs(filter: (registration: string, tab: string) => boolean = () => true): string[] {
  return Object.values(useEditors.getState().tabs)
    .filter((tab) => tab.tx !== null && filter(tab.registration, tab.id))
    .map((tab) => tab.id);
}

/** Does `action` now if no transaction stands in the way; asks otherwise */
export function request(action: PendingAction): void {
  const tabs = affected(action);
  if (tabs.length === 0) void perform(action);
  else useStore.getState().openDialog({ type: "pending", tabs, action });
}

function affected(action: PendingAction): string[] {
  switch (action.kind) {
    case "close":
      return activeTabs((_, tab) => tab === action.tab);
    case "disconnect":
      return activeTabs((registration) => registration === action.id);
    case "quit":
      return activeTabs();
  }
}

/** Ends every affected transaction the same way, then does `action`. A
 *  transaction that fails to end keeps the dialog open, with the error. */
export async function resolve(action: PendingAction, how: "commit" | "rollback"): Promise<string | null> {
  const editors = useEditors.getState();
  for (const tab of affected(action)) await editors.end(tab, how);
  const failed = affected(action);
  if (failed.length > 0) {
    const last = useEditors.getState().tabs[failed[0]]?.messages.at(-1);
    return last?.level === "error" ? last.text : `${panelTitle(failed[0])}: the transaction is still active.`;
  }
  await perform(action);
  return null;
}

async function perform(action: PendingAction) {
  switch (action.kind) {
    case "close":
      closePanel(action.tab);
      break;
    case "disconnect":
      await useStore.getState().disconnect(action.id, true);
      break;
    case "quit":
      await getCurrentWindow().destroy();
      break;
  }
}

/** Asks before the window closes, when it would end transactions */
export async function guardQuit(): Promise<void> {
  try {
    await getCurrentWindow().onCloseRequested((event) => {
      if (activeTabs().length === 0) return;
      event.preventDefault();
      request({ kind: "quit" });
    });
  } catch {
    // Outside Tauri (a browser preview) there is no window to guard
  }
}
