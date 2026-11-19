// The editors that were open come back when the app does: their text, the
// file each is of, and which was in front. Their transactions and results
// do not, and neither do their connections: an editor comes back on a
// database that is connected again by hand.
import { api, errorText, type Session, type SessionPage, type SessionTab } from "./api";
import { activatePanel, baseName, editorPanels, editorTitle, openEditor } from "./dock";
import { textOf, useEditors, type PageSeed } from "./editors";
import { t } from "./i18n";
import { useStore } from "./store";

/** Nothing is written before what was there is read: an early write would
 *  replace the session with the empty window it found */
let restored = false;
let started = false;
let timer: ReturnType<typeof setTimeout> | null = null;
/** What was last written, so that nothing is written twice */
let written = "";

function current(): Session {
  const { ids, active } = editorPanels();
  const tabs: SessionTab[] = [];
  let front: number | null = null;
  for (const id of ids) {
    const tab = useEditors.getState().tabs[id];
    if (!tab || tab.viewer) continue;
    const pages: SessionPage[] = [];
    let page = 0;
    for (const p of tab.pages) {
      const text = !p.file || p.changed ? textOf(p.id) : null;
      // A page nothing was typed in is not worth bringing back
      if (!p.file && !text?.trim()) continue;
      if (p.id === tab.page) page = pages.length;
      pages.push({ file: p.file, format: p.format, text });
    }
    if (pages.length === 0) continue;
    if (id === active) front = tabs.length;
    tabs.push({ registration: tab.registration, title: editorTitle(id), pages, page });
  }
  return { tabs, active: front };
}

/** Writes the session now. Failing is told, once per failure, and tried
 *  again at the next change. */
export async function flushSession(): Promise<void> {
  if (timer) clearTimeout(timer);
  timer = null;
  if (!restored) return;
  const session = current();
  const json = JSON.stringify(session);
  if (json === written) return;
  try {
    await api.sessionSave(session);
    written = json;
  } catch (e) {
    useStore.getState().setNotice(errorText(e));
  }
}

/** Something a session holds changed: it is written once things settle */
export function sessionChanged(): void {
  if (!restored) return;
  if (timer) clearTimeout(timer);
  timer = setTimeout(() => void flushSession(), 1000);
}

/** Opens the editors the last run left */
export async function restoreSession(): Promise<void> {
  if (started) return;
  started = true;
  useEditors.subscribe(watch);
  try {
    const session = await api.sessionLoad();
    written = JSON.stringify(session);
    const failed: string[] = [];
    let front: string | null = null;
    for (const [i, tab] of session.tabs.entries()) {
      const seeds: PageSeed[] = [];
      let page = 0;
      for (const [n, p] of tab.pages.entries()) {
        const seed = await pageSeed(p, failed);
        if (!seed) continue;
        if (n === tab.page) page = seeds.length;
        seeds.push(seed);
      }
      if (seeds.length === 0) continue;
      const id = openEditor(tab.registration, undefined, { pages: seeds, front: page, title: tab.title, restored: true });
      if (id && i === session.active) front = id;
    }
    if (front) activatePanel(front);
    if (failed.length > 0) {
      useStore.getState().setNotice(t("Not opened again, the file is gone: {files}", { files: failed.join(", ") }));
    }
  } catch (e) {
    useStore.getState().setNotice(errorText(e));
  } finally {
    restored = true;
  }
}

/** What a kept page opens with: its text, or its file's when it was the
 *  file's unchanged. Null, and the file named in `failed`, when the file is
 *  gone since. */
async function pageSeed(page: SessionPage, failed: string[]): Promise<PageSeed | null> {
  let text = page.text;
  if (text === null && page.file) {
    try {
      text = (await api.scriptRead(page.file)).text;
    } catch {
      // Moved or deleted since: there is nothing of it to bring back
      failed.push(baseName(page.file));
      return null;
    }
  }
  const file = page.file && page.format ? { file: page.file, format: page.format, changed: page.text !== null } : undefined;
  return { text: text ?? "", file };
}

/** An editor opened or closed, a file saved, a text first changed */
function watch(state: ReturnType<typeof useEditors.getState>, before: ReturnType<typeof useEditors.getState>) {
  if (state.tabs === before.tabs) return;
  const ids = Object.keys(state.tabs);
  const same =
    ids.length === Object.keys(before.tabs).length &&
    ids.every((id) => {
      const [a, b] = [state.tabs[id], before.tabs[id]];
      return b && a.pages === b.pages && a.page === b.page;
    });
  if (!same) sessionChanged();
}
