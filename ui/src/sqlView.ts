// What every SQL text view is built from, the editors' and the Script
// Executive's: the dialect, the look, the keys, completion from the
// catalog of the database the view is on.
import { closeBrackets, closeBracketsKeymap, completionKeymap } from "@codemirror/autocomplete";
import { defaultKeymap, history, historyKeymap, indentWithTab } from "@codemirror/commands";
import { bracketMatching, indentOnInput } from "@codemirror/language";
import { highlightSelectionMatches, searchKeymap } from "@codemirror/search";
import { EditorState, type Extension } from "@codemirror/state";
import { drawSelection, highlightActiveLine, highlightActiveLineGutter, keymap, lineNumbers } from "@codemirror/view";
import { catalogCompletion, completionList } from "./complete";
import { editorLook, firebirdSql } from "./firebird";

/** `registration` is the database completion reads the catalog of, when
 *  there is one */
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
    editorLook,
    EditorState.tabSize.of(2),
    keymap.of([...closeBracketsKeymap, ...defaultKeymap, ...searchKeymap, ...historyKeymap, ...completionKeymap, indentWithTab]),
  ];
}
