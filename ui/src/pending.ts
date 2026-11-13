// Editors with an active transaction are never closed behind the user's
// back: closing one, disconnecting its database or quitting asks first
// whether to commit or roll back. Nor are object viewers holding something
// typed and not compiled or applied: those ask whether to discard it.
import { getCurrentWindow } from "@tauri-apps/api/window";
import { closePanel, panelTitle } from "./dock";
import { useEditors } from "./editors";
import { flushSession } from "./session";
import { useStore } from "./store";
import { hasChanges, useViewers } from "./viewers";

export type PendingAction = { kind: "close"; tab: string } | { kind: "disconnect"; id: string } | { kind: "quit" };

/** Editors with an active transaction, optionally only some */
export function activeTabs(filter: (registration: string, tab: string) => boolean = () => true): string[] {
  return Object.values(useEditors.getState().tabs)
    // A viewer's transaction only reads: nothing of the user's ends with it
    .filter((tab) => tab.tx !== null && !tab.viewer && filter(tab.registration, tab.id))
    .map((tab) => tab.id);
}

/** Viewers with changes that closing them would lose, optionally only some */
export function unsavedViewers(filter: (registration: string) => boolean = () => true): string[] {
  return Object.values(useViewers.getState().viewers)
    .filter((viewer) => hasChanges(viewer) && filter(viewer.registration))
    .map((viewer) => viewer.id);
}

/** Does `action` now if nothing stands in the way; asks otherwise: first
 *  about an editor's unsaved file (`anyway` when that was answered), then
 *  about viewers' changes, then about transactions.
 *
 *  Only a closing editor asks about its file, and only when it has one:
 *  quitting and disconnecting keep every editor's text for the next run
 *  (session.ts), and text that was never a file is a scratch pad's. */
export function request(action: PendingAction, anyway = false): void {
  if (action.kind === "close" && !anyway) {
    const tab = useEditors.getState().tabs[action.tab];
    if (tab?.file && tab.changed) {
      useStore.getState().openDialog({ type: "unsaved", tab: action.tab, action });
      return;
    }
  }
  const unsaved =
    action.kind === "close" ? [] : unsavedViewers(action.kind === "disconnect" ? (r) => r === action.id : undefined);
  if (unsaved.length > 0) {
    useStore.getState().openDialog({ type: "discard", viewers: unsaved, action });
    return;
  }
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
      await flushSession();
      await getCurrentWindow().destroy();
      break;
  }
}

/** Asks before the window closes, when it would end transactions */
export async function guardQuit(): Promise<void> {
  try {
    await getCurrentWindow().onCloseRequested(async (event) => {
      if (activeTabs().length === 0 && unsavedViewers().length === 0) {
        // The window closes once this is written
        await flushSession();
        return;
      }
      event.preventDefault();
      request({ kind: "quit" });
    });
  } catch {
    // Outside Tauri (a browser preview) there is no window to guard
  }
}
