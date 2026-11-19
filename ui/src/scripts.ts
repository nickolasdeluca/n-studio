// Scripts as files: a file opened into an editor, an editor saved to one.
// The backend reads and writes them the way they were found (files.rs).
import { open, save } from "@tauri-apps/plugin-dialog";
import { api, errorText, type FileFormat } from "./api";
import { activatePanel, baseName, editorPanels, editorTitle, openEditor } from "./dock";
import { MAX_PAGES, pageOf, textOf, useEditors } from "./editors";
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

/** Shows the page that has `path` open, or opens it: in a new page of the
 *  editor on `registration` in front, or of its first, when there is room;
 *  in a new editor otherwise */
export async function openScript(registration: string, path: string): Promise<void> {
  const editors = useEditors.getState();
  for (const tab of Object.values(editors.tabs)) {
    const page = tab.pages.find((p) => p.file === path);
    if (page) {
      editors.showPage(tab.id, page.id);
      activatePanel(tab.id);
      return;
    }
  }
  try {
    const { text, format } = await api.scriptRead(path);
    const file = { file: path, format, changed: false };
    const { ids, active } = editorPanels();
    const mine = ids.filter((id) => editors.tabs[id]?.registration === registration);
    const into = [active, ...mine].find((id) => id && mine.includes(id) && editors.tabs[id].pages.length < MAX_PAGES);
    if (into && useEditors.getState().addPage(into, { text, file })) activatePanel(into);
    else openEditor(registration, text, { file });
  } catch (e) {
    useStore.getState().setNotice(errorText(e));
  }
}

/** A page as its tab calls it: the name it was given, its file's, or its number */
export function pageName(page: { n: number; title: string | null; file: string | null }): string {
  return page.title ?? (page.file ? baseName(page.file) : t("Query {n}", { n: page.n }));
}

/** Writes a page's text to its file, the page in front's when `pageId` is
 *  not given, asking where when it has none or `as` says to. False when
 *  nothing was written. */
export async function saveEditor(id: string, as = false, pageId?: string): Promise<boolean> {
  const tab = useEditors.getState().tabs[id];
  const page = tab && pageOf(tab, pageId);
  if (!tab || !page) return false;
  let path = as ? null : page.file;
  try {
    if (!path) {
      const name = tab.pages.length > 1 ? `${editorTitle(id)} ${pageName(page)}` : editorTitle(id);
      path = await save({
        title: t("Save Script"),
        defaultPath: page.file ?? `${name.replace(/[\\/:*?"<>|·]/g, "_").replace(/\s+/g, " ")}.sql`,
        filters: filters(),
      });
      if (!path) return false;
    }
    const format = page.format ?? NEW;
    const written = await api.scriptWrite(path, textOf(page.id), format);
    const editors = useEditors.getState();
    editors.stored(id, page.id, path, written);
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
