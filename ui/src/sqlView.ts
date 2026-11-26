// What every SQL text view is built from, the editors' and the Script
// Executive's: the dialect, the look, the keys, completion from the
// catalog of the database the view is on.
import { closeBrackets, closeBracketsKeymap, completionKeymap, startCompletion } from "@codemirror/autocomplete";
import { defaultKeymap, history, historyKeymap, indentWithTab } from "@codemirror/commands";
import { bracketMatching, indentOnInput } from "@codemirror/language";
import { highlightSelectionMatches, searchKeymap } from "@codemirror/search";
import { EditorState, type Extension } from "@codemirror/state";
import { drawSelection, EditorView, highlightActiveLine, highlightActiveLineGutter, keymap, lineNumbers } from "@codemirror/view";
import { catalogCompletion, completionList, objectAt, objectLinks } from "./complete";
import { openObject } from "./dock";
import { editorLook, firebirdSql } from "./firebird";
import { editorKeys } from "./keys";

/** A double click on the name of something the database has opens it, as
 *  the explorer does; the word is still selected, as on any other. Those
 *  names are drawn as links (`objectLinks`). */
function openOnDoubleClick(registration: () => string | undefined | null): Extension {
  return EditorView.domEventHandlers({
    dblclick(event, view) {
      const id = registration();
      const pos = view.posAtCoords({ x: event.clientX, y: event.clientY });
      if (!id || pos === null || event.shiftKey || event.altKey || event.ctrlKey || event.metaKey) return false;
      void objectAt(view.state, pos, id).then((object) => object && openObject(id, object.kind, object.name));
      return false;
    },
  });
}

/** `registration` is the database completion reads the catalog of, and
 *  double-clicked names are opened in, when there is one */
export function sqlExtensions(registration: () => string | undefined | null): Extension[] {
  return [
    lineNumbers(),
    highlightActiveLineGutter(),
    highlightActiveLine(),
    history(),
    drawSelection(),
    indentOnInput(),
    bracketMatching(),
    closeBrackets(),
    completionList,
    highlightSelectionMatches(),
    firebirdSql,
    catalogCompletion(() => registration() ?? undefined),
    openOnDoubleClick(registration),
    objectLinks(registration),
    editorLook,
    EditorState.tabSize.of(2),
    textKeys,
    keymap.of(textKeymap),
  ];
}

/** Completion's keys but the one that opens it, which is a command of the
 *  app's and has the key the preferences give it */
const completing = completionKeymap.filter((b) => b.run !== startCompletion);

/** CodeMirror's own keys for editing text, which stay as they are */
export const textKeymap = [...closeBracketsKeymap, ...defaultKeymap, ...searchKeymap, ...historyKeymap, ...completing, indentWithTab];

/** The commands of any SQL text */
export const textKeys = editorKeys("text", (view) => ({ "text.complete": () => startCompletion(view) }));
