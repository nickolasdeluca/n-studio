// What the toolbar, the menus and the keyboard can do to a registration or
// to its objects. One place, so each surface offers the same thing under the
// same name.
import { api, errorText, type ObjectRef } from "./api";
import type { MenuItem } from "./components/ContextMenu";
import { openEditor, openObject } from "./dock";
import { t } from "./i18n";
import { useStore } from "./store";

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
  ];
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
