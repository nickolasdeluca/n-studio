// What the toolbar, the menus and the keyboard can do to a registration.
// One place, so each surface offers the same thing under the same name.
import type { MenuItem } from "./components/ContextMenu";
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
    {
      label: t("Edit Registration…"),
      icon: "edit",
      onSelect: () => s.openDialog({ type: "registration", registration }),
    },
    { label: t("Unregister Database…"), icon: "trash", danger: true, onSelect: () => s.openDialog({ type: "delete", id }) },
  ];
}
