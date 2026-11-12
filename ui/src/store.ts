// View state only (PLAN §7): what is expanded, selected, loading, and the
// last answers from the backend. Connections and metadata live in Rust.
import { create } from "zustand";
import {
  api,
  errorText,
  isAppError,
  type DbObject,
  type Form,
  type ObjectKind,
  type Registration,
  type RegistrationView,
  type ServerInfo,
  type TreeKind,
} from "./api";
import { onMetadataChanged } from "./changes";
import { useEditors } from "./editors";
import { t } from "./i18n";
import type { ParamRequest } from "./params";
import { activeTabs, unsavedViewers, type PendingAction } from "./pending";

interface Loadable<T> {
  data?: T;
  loading: boolean;
  error?: string;
}

/** A tree row, by key: `r:<id>`, `k:<id>:<kind>`, `o:<id>:<kind>:<name>` */
export type NodeKey = string;

export const keys = {
  registration: (id: string) => `r:${id}`,
  kind: (id: string, kind: ObjectKind) => `k:${id}:${kind}`,
  object: (id: string, kind: ObjectKind, name: string) => `o:${id}:${kind}:${name}`,
};

export type Dialog =
  | { type: "registration"; registration: Registration | null }
  | { type: "password"; id: string; error?: string }
  | { type: "delete"; id: string }
  /** Editors whose transactions an action would end: commit or roll back? */
  | { type: "pending"; tabs: string[]; action: PendingAction }
  /** A statement's parameter values, which a run waits for */
  | { type: "params"; request: ParamRequest }
  /** Where and how to write an editor's rows */
  | { type: "export"; tab: string }
  /** A change to the catalog, as the script that makes it: shown, then run
   *  whole or not at all. `done` is called once it is committed.
   *  `back` is the dialog it came from, shown again if it is cancelled;
   *  `verb` names the button when the script compiles nothing. */
  | { type: "compile"; registration: string; title: string; script: string; danger?: boolean; done?: () => void; back?: Dialog; verb?: string }
  /** What an action asks before it goes on: usually to a script to compile */
  | {
      type: "form";
      title: string;
      form: Form;
      /** What was typed, when the dialog comes back from what it led to */
      values?: Record<string, string>;
      /** Goes on with the values, or fails with why it cannot; `back` is
       *  this dialog with them, for whatever comes next to return to */
      submit(values: Record<string, string>, back: Dialog): Promise<void>;
      /** What happens next, under the fields; the script is shown, unless said otherwise */
      note?: string;
      /** The button's name: "Continue…" unless said otherwise */
      verb?: string;
    }
  /** Viewers closing over what was typed and not compiled or applied;
   *  `action` is what goes on once it is discarded, when it is not just
   *  the viewers closing */
  | { type: "discard"; viewers: string[]; action?: PendingAction };

/** What the tree has selected: a registration, and maybe a kind and object */
export interface Selection {
  id: string;
  kind?: ObjectKind;
  object?: string;
}

interface State {
  registrations: RegistrationView[];
  charsets: string[];
  /** Connected registrations' servers, by id */
  servers: Record<string, ServerInfo>;
  connecting: Record<string, boolean>;
  trees: Record<string, Loadable<TreeKind[]>>;
  /** By `<id>:<kind>` */
  objects: Record<string, Loadable<DbObject[]>>;
  expanded: Record<NodeKey, boolean>;
  selection: Selection | null;
  dialog: Dialog | null;
  /** The last thing that went wrong outside a dialog */
  notice: string | null;
  /** What is being waited for outside a panel of its own */
  busy: string | null;

  load(): Promise<void>;
  select(selection: Selection | null): void;
  toggle(key: NodeKey, open?: boolean): void;
  connect(id: string, password?: string, remember?: boolean): Promise<void>;
  /** Asks first when editors on it have active transactions, unless `force` */
  disconnect(id: string, force?: boolean): Promise<void>;
  refresh(id: string): Promise<void>;
  loadObjects(id: string, kind: ObjectKind): Promise<void>;
  saveRegistration(registration: Registration, password: string | null): Promise<Registration>;
  deleteRegistration(id: string): Promise<void>;
  openDialog(dialog: Dialog | null): void;
  setNotice(notice: string | null): void;
  setBusy(busy: string | null): void;
}

const objectsKey = (id: string, kind: ObjectKind) => `${id}:${kind}`;

/** Everything cached for a registration's connection, dropped on disconnect */
function forget<T>(map: Record<string, T>, id: string): Record<string, T> {
  return Object.fromEntries(Object.entries(map).filter(([k]) => !k.startsWith(`${id}:`)));
}

export const useStore = create<State>()((set, get) => ({
  registrations: [],
  charsets: [],
  servers: {},
  connecting: {},
  trees: {},
  objects: {},
  expanded: {},
  selection: null,
  dialog: null,
  notice: null,
  busy: null,

  async load() {
    try {
      const [registrations, charsets] = await Promise.all([api.listRegistrations(), api.charsets()]);
      const servers: Record<string, ServerInfo> = {};
      for (const r of registrations) if (r.server) servers[r.id] = r.server;
      set({ registrations, charsets, servers });
      // Still connected after a reload of the window
      for (const id of Object.keys(servers)) void loadTree(id);
    } catch (e) {
      set({ notice: errorText(e) });
    }
  },

  select(selection) {
    set({ selection });
    if (selection?.kind && get().servers[selection.id]) void get().loadObjects(selection.id, selection.kind);
  },

  toggle(key, open) {
    set((s) => ({ expanded: { ...s.expanded, [key]: open ?? !s.expanded[key] } }));
  },

  async connect(id, password, remember = false) {
    if (get().connecting[id]) return;
    set((s) => ({ connecting: { ...s.connecting, [id]: true } }));
    try {
      const server = await api.connect(id, password ?? null, remember);
      set((s) => ({
        servers: { ...s.servers, [id]: server },
        expanded: { ...s.expanded, [keys.registration(id)]: true },
        dialog: s.dialog?.type === "password" && s.dialog.id === id ? null : s.dialog,
        notice: null,
      }));
      if (remember) void refreshRegistrations();
      await loadTree(id);
    } catch (e) {
      if (isAppError(e) && e.kind === "password_required") {
        set({ dialog: { type: "password", id } });
      } else if (get().dialog?.type === "password") {
        set({ dialog: { type: "password", id, error: errorText(e) } });
      } else {
        set({ notice: t("Could not connect: {error}", { error: errorText(e) }) });
      }
    } finally {
      set((s) => ({ connecting: { ...s.connecting, [id]: false } }));
    }
  },

  async disconnect(id, force = false) {
    const unsaved = force ? [] : unsavedViewers((registration) => registration === id);
    if (unsaved.length > 0) {
      set({ dialog: { type: "discard", viewers: unsaved, action: { kind: "disconnect", id } } });
      return;
    }
    if (!force && activeTabs((registration) => registration === id).length > 0) {
      set({ dialog: { type: "pending", tabs: activeTabs((r) => r === id), action: { kind: "disconnect", id } } });
      return;
    }
    await api.disconnect(id);
    useEditors.getState().sessionClosed(id);
    set((s) => {
      const servers = { ...s.servers };
      delete servers[id];
      const trees = { ...s.trees };
      delete trees[id];
      return { servers, trees, objects: forget(s.objects, id) };
    });
  },

  async refresh(id) {
    set((s) => ({ objects: forget(s.objects, id) }));
    await loadTree(id);
    const sel = get().selection;
    if (sel?.id === id && sel.kind) await get().loadObjects(id, sel.kind);
    // Expanded kinds show their objects in the tree: reload those too
    for (const key of Object.keys(get().expanded)) {
      const [tag, rid, kind] = key.split(":");
      if (tag === "k" && rid === id && get().expanded[key] && kind !== sel?.kind) void get().loadObjects(id, kind);
    }
  },

  async loadObjects(id, kind) {
    const key = objectsKey(id, kind);
    const current = get().objects[key];
    if (current?.loading || current?.data) return;
    set((s) => ({ objects: { ...s.objects, [key]: { loading: true } } }));
    try {
      const data = await api.objectList(id, kind);
      set((s) => ({ objects: { ...s.objects, [key]: { data, loading: false } } }));
    } catch (e) {
      set((s) => ({ objects: { ...s.objects, [key]: { loading: false, error: errorText(e) } } }));
    }
  },

  async saveRegistration(registration, password) {
    const saved = await api.saveRegistration(registration, password);
    await refreshRegistrations();
    return saved;
  },

  async deleteRegistration(id) {
    await api.deleteRegistration(id);
    useEditors.getState().sessionClosed(id);
    set((s) => {
      const servers = { ...s.servers };
      delete servers[id];
      return {
        servers,
        objects: forget(s.objects, id),
        selection: s.selection?.id === id ? null : s.selection,
      };
    });
    await refreshRegistrations();
  },

  openDialog(dialog) {
    // A run waiting for its parameters must not wait forever
    const replaced = get().dialog;
    if (replaced?.type === "params" && replaced !== dialog) replaced.request.done(null);
    set({ dialog });
  },

  setNotice(notice) {
    set({ notice });
  },

  setBusy(busy) {
    set({ busy });
  },
}));

export function objectsOf(id: string, kind: ObjectKind): Loadable<DbObject[]> | undefined {
  return useStore.getState().objects[objectsKey(id, kind)];
}

export function useObjects(id: string | undefined, kind: ObjectKind | undefined) {
  return useStore((s) => (id && kind ? s.objects[objectsKey(id, kind)] : undefined));
}

async function refreshRegistrations() {
  try {
    useStore.setState({ registrations: await api.listRegistrations() });
  } catch (e) {
    useStore.setState({ notice: errorText(e) });
  }
}

async function loadTree(id: string) {
  useStore.setState((s) => ({ trees: { ...s.trees, [id]: { ...s.trees[id], loading: true, error: undefined } } }));
  try {
    const data = await api.explorerTree(id);
    useStore.setState((s) => ({ trees: { ...s.trees, [id]: { data, loading: false } } }));
  } catch (e) {
    useStore.setState((s) => ({ trees: { ...s.trees, [id]: { loading: false, error: errorText(e) } } }));
  }
}

// What changed the catalog says so; the tree and the lists read it again
onMetadataChanged((id) => {
  if (useStore.getState().servers[id]) void useStore.getState().refresh(id);
});
