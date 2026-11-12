// The object viewers' view state: which tabs an object has, the one showing,
// and what the backend answered for each. A tab's content is asked for when
// the tab is first shown, and again on a reload.
import { create } from "zustand";
import { api, errorText, type ObjectKind, type TabContent, type TabInfo } from "./api";
import { useEditors } from "./editors";

interface Loadable<T> {
  data?: T;
  loading: boolean;
  error?: string;
}

export interface Viewer {
  /** Its panel's id too: one viewer per object */
  id: string;
  registration: string;
  kind: ObjectKind;
  name: string;
  tabs: Loadable<TabInfo[]>;
  /** The tab showing; `null` until the tabs are known */
  active: string | null;
  /** By tab id */
  contents: Record<string, Loadable<TabContent>>;
  /** The editor tab the Data tab's rows come through, once it was shown */
  data: string | null;
  /** Text changed and not compiled yet, by tab id */
  drafts: Record<string, string>;
}

interface State {
  viewers: Record<string, Viewer>;
  /** The viewer of an object, made if there is none; returns its id */
  open(registration: string, kind: ObjectKind, name: string): string;
  close(id: string): void;
  show(id: string, tab: string): void;
  /** Reads the tab showing again; what was typed in it stays */
  reload(id: string): void;
  /** The object changed: the tab showing is read again, the others when
   *  they are next shown */
  refresh(id: string): void;
  /** What a tab's text was changed to; `null` when it is the object's again */
  setDraft(id: string, tab: string, text: string | null): void;
}

export const viewerId = (registration: string, kind: ObjectKind, name: string) => `obj-${registration}:${kind}:${name}`;

export const useViewers = create<State>()((set, get) => {
  const patch = (id: string, p: Partial<Viewer> | ((viewer: Viewer) => Partial<Viewer>)) =>
    set((s) => {
      const viewer = s.viewers[id];
      if (!viewer) return s;
      return { viewers: { ...s.viewers, [id]: { ...viewer, ...(typeof p === "function" ? p(viewer) : p) } } };
    });
  const setContent = (id: string, tab: string, content: Loadable<TabContent>) =>
    patch(id, (v) => ({ contents: { ...v.contents, [tab]: content } }));

  /** Runs the Data tab's query, in a read-only transaction of its own */
  const runData = (id: string, sql: string) => {
    const viewer = get().viewers[id];
    if (!viewer) return;
    const editors = useEditors.getState();
    const data = viewer.data ?? editors.open(viewer.registration, true);
    if (!viewer.data) patch(id, { data });
    void editors.run(data, "all", { text: sql, from: 0, to: 0 });
  };

  const load = async (id: string, tab: string) => {
    const viewer = get().viewers[id];
    if (!viewer || viewer.contents[tab]?.loading) return;
    setContent(id, tab, { ...viewer.contents[tab], loading: true, error: undefined });
    try {
      const data = await api.objectTab(viewer.registration, viewer.kind, viewer.name, tab);
      setContent(id, tab, { data, loading: false });
      if (data.shape === "data") runData(id, data.sql);
    } catch (e) {
      setContent(id, tab, { loading: false, error: errorText(e) });
    }
  };

  return {
    viewers: {},

    open(registration, kind, name) {
      const id = viewerId(registration, kind, name);
      if (get().viewers[id]) return id;
      const viewer: Viewer = {
        id,
        registration,
        kind,
        name,
        tabs: { loading: true },
        active: null,
        contents: {},
        data: null,
        drafts: {},
      };
      set((s) => ({ viewers: { ...s.viewers, [id]: viewer } }));
      api.objectTabs(kind).then(
        (tabs) => {
          patch(id, { tabs: { data: tabs, loading: false } });
          if (tabs.length > 0) get().show(id, tabs[0].id);
        },
        (e) => patch(id, { tabs: { loading: false, error: errorText(e) } }),
      );
      return id;
    },

    close(id) {
      const viewer = get().viewers[id];
      if (!viewer) return;
      set((s) => {
        const viewers = { ...s.viewers };
        delete viewers[id];
        return { viewers };
      });
      if (viewer.data) void useEditors.getState().close(viewer.data);
    },

    show(id, tab) {
      const viewer = get().viewers[id];
      if (!viewer) return;
      patch(id, { active: tab });
      const content = viewer.contents[tab];
      if (!content?.data && !content?.loading) void load(id, tab);
    },

    reload(id) {
      const viewer = get().viewers[id];
      if (viewer?.active) void load(id, viewer.active);
    },

    refresh(id) {
      const viewer = get().viewers[id];
      if (!viewer) return;
      const active = viewer.active;
      // What the tab showing has stays on screen while it is read again
      patch(id, { contents: active && viewer.contents[active] ? { [active]: viewer.contents[active] } : {} });
      if (active) void load(id, active);
    },

    setDraft(id, tab, text) {
      patch(id, (v) => {
        const drafts = { ...v.drafts };
        if (text === null) delete drafts[tab];
        else drafts[tab] = text;
        return { drafts };
      });
    },
  };
});
