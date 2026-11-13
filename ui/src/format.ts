// The code formatter, from the editor's side: the style in use, and laying
// out an editor's text by it. The formatting itself is the backend's
// (fb-sql), which changes white space and the case of words and nothing
// else.
import type { EditorView } from "@codemirror/view";
import { api, errorText, type FormatStyle } from "./api";
import { t } from "./i18n";
import { useStore } from "./store";

/** The saved style, read once and kept */
let current: Promise<FormatStyle> | null = null;

export function formatStyle(): Promise<FormatStyle> {
  return (current ??= api.formatStyleLoad());
}

/** Makes `style` the one in use, here and in the file that keeps it */
export async function saveFormatStyle(style: FormatStyle): Promise<void> {
  await api.formatStyleSave(style);
  current = Promise.resolve(style);
}

/**
 * Formats what is selected in the editor, or all of its text when nothing
 * is. One change, so one undo takes it back.
 */
export async function formatEditor(view: EditorView): Promise<void> {
  const selection = view.state.selection.main;
  const from = selection.empty ? 0 : selection.from;
  const to = selection.empty ? view.state.doc.length : selection.to;
  const text = view.state.sliceDoc(from, to);
  if (!text.trim()) return;
  try {
    const formatted = await api.formatSql(text, await formatStyle());
    // Typed in meanwhile: what came back is not this text's
    if (view.state.sliceDoc(from, to) !== text) return;
    if (formatted === text) {
      useStore.getState().setNotice(t("Nothing to format: the text is as the style has it, or ends inside a string or a comment."));
      return;
    }
    view.dispatch({
      changes: { from, to, insert: formatted },
      selection: selection.empty
        ? { anchor: Math.min(selection.head, formatted.length) }
        : { anchor: from, head: from + formatted.length },
      userEvent: "input.format",
      scrollIntoView: true,
    });
  } catch (e) {
    useStore.getState().setNotice(errorText(e));
  }
}
