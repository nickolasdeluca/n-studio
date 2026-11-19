// What every SQL text view is built from, the editors' and the Script
// Executive's: the dialect, the look, the keys, completion from the
// catalog of the database the view is on.
import { closeBrackets, closeBracketsKeymap, completionKeymap } from "@codemirror/autocomplete";
import { defaultKeymap, history, historyKeymap, indentWithTab } from "@codemirror/commands";
import { bracketMatching, indentOnInput } from "@codemirror/language";
import { highlightSelectionMatches, searchKeymap } from "@codemirror/search";
import { EditorState, type Extension } from "@codemirror/state";
import { drawSelection, EditorView, highlightActiveLine, highlightActiveLineGutter, keymap, lineNumbers } from "@codemirror/view";
import { catalogCompletion, completionList, objectAt } from "./complete";
import { openObject } from "./dock";
import { editorLook, firebirdSql } from "./firebird";

/** A double click on the name of something the database has opens it, as
 *  the explorer does; the word is still selected, as on any other */
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
    editorLook,
    EditorState.tabSize.of(2),
    keymap.of([...closeBracketsKeymap, ...defaultKeymap, ...searchKeymap, ...historyKeymap, ...completionKeymap, indentWithTab]),
  ];
}
