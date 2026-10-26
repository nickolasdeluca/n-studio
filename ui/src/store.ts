// View state only (PLAN §7): what is expanded, selected, loading, and the
// last answers from the backend. Connections and metadata live in Rust.
import { create } from "zustand";
import {
  api,
  errorText,
  isAppError,
  type DbObject,
  type ObjectKind,
  type Registration,
  type RegistrationView,
  type ServerInfo,
  type TreeKind,
} from "./api";
import { t } from "./i18n";

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
  | { type: "delete"; id: string };

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

  load(): Promise<void>;
  select(selection: Selection | null): void;
  toggle(key: NodeKey, open?: boolean): void;
  connect(id: string, password?: string, remember?: boolean): Promise<void>;
  disconnect(id: string): Promise<void>;
  refresh(id: string): Promise<void>;
  loadObjects(id: string, kind: ObjectKind): Promise<void>;
  saveRegistration(registration: Registration, password: string | null): Promise<Registration>;
  deleteRegistration(id: string): Promise<void>;
  openDialog(dialog: Dialog | null): void;
  setNotice(notice: string | null): void;
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

  async disconnect(id) {
    await api.disconnect(id);
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
    set({ dialog });
  },

  setNotice(notice) {
    set({ notice });
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
