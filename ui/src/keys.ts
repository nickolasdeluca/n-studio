// The keymap as the app uses it: which command a key press runs, where the
// keyboard is, with the keys the preferences have now.
import { Prec } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { commandsFor, display, keysOf, pressed, type KeyPress, type Scope } from "./keymap";
import { usePrefs } from "./prefs";

export const isMac = /Mac|iPhone|iPad/.test(navigator.platform);

/** What a place does for its commands, by command id: true when it did
 *  something, false to leave the key to whatever else wants it */
export type Handlers = Record<string, () => boolean | void>;

/** Runs the command of the key pressed that `handlers` has, the first one
 *  the key is for. Whether one ran: the key is then taken. */
export function runKey(scope: Scope, e: KeyPress & { preventDefault(): void }, handlers: Handlers): boolean {
  const key = pressed(e);
  if (!key) return false;
  for (const c of commandsFor(key, scope, usePrefs.getState().keys, isMac)) {
    const run = handlers[c.id];
    if (run && run() !== false) {
      e.preventDefault();
      return true;
    }
  }
  return false;
}

/** The commands of a CodeMirror editor, ahead of its own keys */
export function editorKeys(scope: Scope, handlers: (view: EditorView) => Handlers) {
  return Prec.highest(EditorView.domEventHandlers({ keydown: (e, view) => runKey(scope, e, handlers(view)) }));
}

/** A command's first key, as the keyboard says it, for a tooltip: "" when it
 *  has none */
export function shortcut(id: string): string {
  const key = keysOf(id, usePrefs.getState().keys)[0];
  return key ? display(key, isMac) : "";
}

/** A label with its command's key after it: "Save (⌘S)" */
export function withKey(label: string, id: string): string {
  const key = shortcut(id);
  return key ? `${label} (${key})` : label;
}

/** Re-renders when the keys change, so tooltips show the new ones */
export const useKeys = () => usePrefs((s) => s.keys);
