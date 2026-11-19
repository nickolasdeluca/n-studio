// Each database's workspace, as IBExpert keeps it: the SQL editors open on
// it, every page of their text, the file each page is of, and which editor
// and page were in front. A database's editors close when it disconnects
// and come back when it connects, in this run of the app or a later one;
// their transactions and results do not come back. Object viewers close
// with their database too, and do not come back.
import { api, errorText, type Session, type SessionPage, type SessionTab } from "./api";
import { activatePanel, baseName, closePanel, editorPanels, editorTitle, openEditor } from "./dock";
import { textOf, useEditors, type EditorTab, type PageSeed } from "./editors";
import { t } from "./i18n";
import { useStore } from "./store";
import { useViewers } from "./viewers";

/** Nothing is written before what was there is read: an early write would
 *  replace the session with the empty window it found */
let restored = false;
let started = false;
let timer: ReturnType<typeof setTimeout> | null = null;
/** What was last written, so that nothing is written twice */
let written = "";
/** The editors of the databases that are not connected, as they were left */
let parked: SessionTab[] = [];
/** The editor last in front, by registration */
const lastFront = new Map<string, string>();

/** An editor as it is kept: null when no page of it has anything */
function kept(tab: EditorTab, front: boolean): SessionTab | null {
  const pages: SessionPage[] = [];
  let page = 0;
  for (const p of tab.pages) {
    const text = !p.file || p.changed ? textOf(p.id) : null;
    // A page nothing was typed in is not worth bringing back
    if (!p.file && !text?.trim()) continue;
    if (p.id === tab.page) page = pages.length;
    pages.push({ file: p.file, format: p.format, text });
  }
  return pages.length === 0 ? null : { registration: tab.registration, title: editorTitle(tab.id), pages, page, front };
}

/** The open editors of a registration, or of every one, as they are kept */
function open(registration?: string): SessionTab[] {
  const { ids, active } = editorPanels();
  const editors = useEditors.getState().tabs;
  const front = editors[active ?? ""];
  if (front) lastFront.set(front.registration, front.id);
  const tabs: SessionTab[] = [];
  for (const id of ids) {
    const tab = editors[id];
    if (!tab || tab.viewer || (registration !== undefined && tab.registration !== registration)) continue;
    const one = kept(tab, lastFront.get(tab.registration) === id);
    if (one) tabs.push(one);
  }
  return tabs;
}

function current(): Session {
  const live = open();
  // A registration deleted since takes its workspace with it
  const known = new Set(useStore.getState().registrations.map((r) => r.id));
  const away = parked.filter((tab) => known.size === 0 || known.has(tab.registration));
  return { tabs: [...live, ...away] };
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

/** Reads the workspaces the last run left, and opens those of the
 *  databases already connected */
export async function restoreSession(): Promise<void> {
  if (started) return;
  started = true;
  useEditors.subscribe(watch);
  useStore.subscribe(connections);
  try {
    const session = await api.sessionLoad();
    written = JSON.stringify(session);
    parked = session.tabs;
  } catch (e) {
    useStore.getState().setNotice(errorText(e));
  } finally {
    restored = true;
  }
  for (const id of Object.keys(useStore.getState().servers)) await unpark(id);
}

/** A database connected: its editors come back. One disconnected: they are
 *  kept, and close with its object viewers. */
function connections(state: ReturnType<typeof useStore.getState>, before: ReturnType<typeof useStore.getState>) {
  if (!restored || state.servers === before.servers) return;
  for (const id of Object.keys(before.servers)) if (!state.servers[id]) park(id);
  for (const id of Object.keys(state.servers)) if (!before.servers[id]) void unpark(id);
}

function park(registration: string) {
  parked = [...parked.filter((tab) => tab.registration !== registration), ...open(registration)];
  for (const id of editorPanels().ids) {
    const tab = useEditors.getState().tabs[id];
    if (tab?.registration === registration) closePanel(id);
  }
  for (const viewer of Object.values(useViewers.getState().viewers)) {
    if (viewer.registration === registration) closePanel(viewer.id);
  }
  void flushSession();
}

async function unpark(registration: string) {
  const tabs = parked.filter((tab) => tab.registration === registration);
  if (tabs.length === 0) return;
  parked = parked.filter((tab) => tab.registration !== registration);
  const failed: string[] = [];
  let front: string | null = null;
  for (const tab of tabs) {
    const seeds: PageSeed[] = [];
    let page = 0;
    for (const [n, p] of tab.pages.entries()) {
      const seed = await pageSeed(p, failed);
      if (!seed) continue;
      if (n === tab.page) page = seeds.length;
      seeds.push(seed);
    }
    // Disconnected again while its files were read: kept as it was
    if (!useStore.getState().servers[registration]) {
      parked = [...parked, tab];
      continue;
    }
    if (seeds.length === 0) continue;
    const id = openEditor(registration, undefined, { pages: seeds, front: page, title: tab.title, restored: true });
    if (id && (tab.front || !front)) front = id;
  }
  if (front) activatePanel(front);
  if (failed.length > 0) {
    useStore.getState().setNotice(t("Not opened again, the file is gone: {files}", { files: failed.join(", ") }));
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

/** An editor opened or closed, a page added, closed or brought to the
 *  front, a file saved, a text first changed */
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
