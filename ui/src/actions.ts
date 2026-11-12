// What the toolbar, the menus and the keyboard can do to a registration or
// to its objects. One place, so each surface offers the same thing under the
// same name.
import { adminActions } from "./admin";
import { api, errorText, type Action, type ObjectKind, type ObjectRef } from "./api";
import type { MenuItem } from "./components/ContextMenu";
import { startComparison } from "./compare";
import { openDiagram } from "./diagram";
import { closePanel, openEditor, openObject } from "./dock";
import { t } from "./i18n";
import { useStore, type Dialog } from "./store";
import { useViewers, viewerId, type Viewer } from "./viewers";

export function registrationActions(id: string): (MenuItem | "separator")[] {
  const s = useStore.getState();
  const connected = !!s.servers[id];
  const registration = s.registrations.find((r) => r.id === id) ?? null;
  return [
    { label: t("Connect"), icon: "plug", disabled: connected || s.connecting[id], onSelect: () => void s.connect(id) },
    { label: t("Disconnect"), icon: "unplug", disabled: !connected, onSelect: () => void s.disconnect(id) },
    { label: t("Refresh"), icon: "refresh", disabled: !connected, onSelect: () => void s.refresh(id) },
    "separator",
    { label: t("New SQL Editor"), icon: "sql", disabled: !connected, onSelect: () => openEditor(id) },
    "separator",
    ...adminActions(id),
    "separator",
    {
      label: t("Edit Registration…"),
      icon: "edit",
      onSelect: () => s.openDialog({ type: "registration", registration }),
    },
    { label: t("Unregister Database…"), icon: "trash", danger: true, onSelect: () => s.openDialog({ type: "delete", id }) },
  ];
}

/** What can be done to the objects picked in the explorer or the object list */
export function objectActions(id: string, objects: ObjectRef[]): (MenuItem | "separator")[] {
  const one = objects.length === 1 ? objects[0] : null;
  const tables = objects.filter((o) => o.kind === "table").map((o) => o.name);
  return [
    {
      label: one ? t("Open") : t("Open {n} Objects", { n: objects.length }),
      icon: "open",
      // A screenful of tabs at once helps nobody
      disabled: objects.length === 0 || objects.length > 12,
      onSelect: () => objects.forEach((o) => openObject(id, o.kind, o.name)),
    },
    {
      label: one ? t("Extract Metadata") : t("Extract Metadata of {n} Objects", { n: objects.length }),
      icon: "sql",
      disabled: objects.length === 0,
      onSelect: () => void extractMetadata(id, objects),
    },
    {
      label: t("Compare Data…"),
      icon: "copy",
      disabled: one?.kind !== "table",
      onSelect: () => one && startComparison(id, one.name),
    },
    {
      label: t("Diagram"),
      icon: "table",
      // Of tables and what they are related to
      disabled: tables.length === 0 || tables.length !== objects.length,
      onSelect: () => openDiagram(id, tables),
    },
    "separator",
    {
      label: one ? t("Drop {name}…", { name: one.name }) : t("Drop…"),
      icon: "trash",
      danger: true,
      // One at a time: what depends on what decides the order of several
      disabled: !one,
      onSelect: () => one && void dropObject(id, one),
    },
  ];
}

/** What can be done to a kind as a whole */
export function kindActions(id: string, kind: ObjectKind, singular: string): (MenuItem | "separator")[] {
  return [{ label: t("New {kind}…", { kind: t(singular) }), icon: "plus", onSelect: () => void newObject(id, kind) }];
}

/** A new SQL editor holding a script that creates an object of the kind */
async function newObject(id: string, kind: ObjectKind) {
  try {
    openEditor(id, await api.objectTemplate(id, kind));
  } catch (e) {
    useStore.getState().setNotice(errorText(e));
  }
}

/** Shows the statement that drops the object, to be confirmed */
async function dropObject(id: string, object: ObjectRef) {
  const s = useStore.getState();
  try {
    const script = await api.objectDropScript(id, object.kind, object.name);
    s.openDialog({
      type: "compile",
      registration: id,
      title: t("Drop {name}", { name: object.name }),
      script,
      danger: true,
      done: () => closePanel(viewerId(id, object.kind, object.name)),
    });
  } catch (e) {
    s.setNotice(errorText(e));
  }
}

/** Starts an action of a viewer's: its form when it asks for values, its
 *  script to confirm otherwise. `key` is the row it acts on, if any. */
export async function startAction(viewer: Viewer, action: Action, key: string | null) {
  const s = useStore.getState();
  try {
    if (action.form) {
      const form = await api.objectActionForm(viewer.registration, viewer.kind, viewer.name, action.id, key);
      s.openDialog({
        type: "form",
        title: `${t(action.label).replace(/…$/, "")}: ${form.title}`,
        form,
        submit: async (values, back) => {
          // The viewer as it is by then: closed meanwhile, there is nothing to do
          const now = useViewers.getState().viewers[viewer.id];
          if (now) await confirmAction(now, action, key, values, back);
        },
      });
    } else {
      await confirmAction(viewer, action, key, {});
    }
  } catch (e) {
    s.setNotice(errorText(e));
  }
}

/** Shows the script that carries an action out, to be compiled. Fails when
 *  the values do not make one. `back` is the form the values came from. */
export async function confirmAction(
  viewer: Viewer,
  action: Action,
  key: string | null,
  values: Record<string, string>,
  back?: Dialog,
) {
  const { registration, kind, name } = viewer;
  const { script, title, renamed } = await api.objectActionScript(registration, kind, name, action.id, key, values);
  useStore.getState().openDialog({
    type: "compile",
    registration,
    title: `${t(action.label).replace(/…$/, "")}: ${title}`,
    script,
    danger: action.danger,
    back,
    done: () => {
      if (renamed === null) {
        useViewers.getState().refresh(viewer.id);
        return;
      }
      // Under its new name it is another object, with another viewer
      closePanel(viewer.id);
      openObject(registration, kind, renamed);
    },
  });
}

/** The script that creates the objects again, in a new SQL editor */
export async function extractMetadata(id: string, objects: ObjectRef[]) {
  const s = useStore.getState();
  s.setBusy(t("Extracting the metadata of {n} object(s)…", { n: objects.length }));
  try {
    openEditor(id, await api.extractMetadata(id, objects));
  } catch (e) {
    s.setNotice(t("Could not extract the metadata: {error}", { error: errorText(e) }));
  } finally {
    s.setBusy(null);
  }
}
