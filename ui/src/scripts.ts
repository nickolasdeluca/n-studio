// Scripts as files: a file opened into an editor, an editor saved to one.
// The backend reads and writes them the way they were found (files.rs).
import { open, save } from "@tauri-apps/plugin-dialog";
import { api, errorText, type FileFormat } from "./api";
import { activatePanel, baseName, openEditor, panelTitle } from "./dock";
import { textOf, useEditors } from "./editors";
import { t } from "./i18n";
import { useStore } from "./store";

const filters = () => [
  { name: t("SQL scripts"), extensions: ["sql", "ddl", "dml", "txt"] },
  { name: t("All files"), extensions: ["*"] },
];

/** What a file that did not exist before is written as */
const NEW: FileFormat = { encoding: "utf8", crlf: false };

const encodings: Record<FileFormat["encoding"], string> = {
  utf8: "UTF-8",
  utf8_bom: "UTF-8 BOM",
  utf16_le: "UTF-16 LE",
  utf16_be: "UTF-16 BE",
  win1252: "WIN1252",
};

/** A format as the status line names it */
export const formatLabel = (format: FileFormat) => `${encodings[format.encoding]} · ${format.crlf ? "CRLF" : "LF"}`;

/** Asks for scripts and opens each in an editor on `registration` */
export async function openScripts(registration: string): Promise<void> {
  let picked: string | string[] | null;
  try {
    picked = await open({ title: t("Open Script"), multiple: true, filters: filters() });
  } catch (e) {
    useStore.getState().setNotice(errorText(e));
    return;
  }
  for (const path of picked === null ? [] : [picked].flat()) await openScript(registration, path);
}

/** Shows the editor that has `path` open, or opens one for it */
export async function openScript(registration: string, path: string): Promise<void> {
  const has = Object.values(useEditors.getState().tabs).find((tab) => tab.file === path);
  if (has) {
    activatePanel(has.id);
    return;
  }
  try {
    const { text, format } = await api.scriptRead(path);
    openEditor(registration, text, { file: { file: path, format, changed: false } });
  } catch (e) {
    useStore.getState().setNotice(errorText(e));
  }
}

/** Writes an editor's text to its file, asking where when it has none or
 *  `as` says to. False when nothing was written. */
export async function saveEditor(id: string, as = false): Promise<boolean> {
  const tab = useEditors.getState().tabs[id];
  if (!tab) return false;
  let path = as ? null : tab.file;
  try {
    if (!path) {
      path = await save({
        title: t("Save Script"),
        defaultPath: tab.file ?? `${panelTitle(id).replace(/[\\/:*?"<>|·]/g, "_").replace(/\s+/g, " ")}.sql`,
        filters: filters(),
      });
      if (!path) return false;
    }
    const format = tab.format ?? NEW;
    const written = await api.scriptWrite(path, textOf(id), format);
    const editors = useEditors.getState();
    editors.stored(id, path, written);
    if (written.encoding !== format.encoding) {
      editors.note(
        id,
        t("{file} was saved as UTF-8: {was} does not have every character of the text.", {
          file: baseName(path),
          was: encodings[format.encoding],
        }),
      );
    }
    return true;
  } catch (e) {
    useStore.getState().setNotice(errorText(e));
    return false;
  }
}
