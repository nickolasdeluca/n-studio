import { useState } from "react";
import { errorText } from "../api";
import { t } from "../i18n";
import { useStore } from "../store";
import { Button, Field, inputClass, Modal } from "./Modal";
import { RegistrationDialog } from "./RegistrationDialog";

/** Whichever dialog the store has open */
export function Dialogs() {
  const dialog = useStore((s) => s.dialog);
  if (!dialog) return null;
  switch (dialog.type) {
    case "registration":
      return <RegistrationDialog registration={dialog.registration} />;
    case "password":
      return <PasswordDialog id={dialog.id} error={dialog.error} />;
    case "delete":
      return <DeleteDialog id={dialog.id} />;
  }
}

function PasswordDialog({ id, error }: { id: string; error?: string }) {
  const registration = useStore((s) => s.registrations.find((r) => r.id === id));
  const connecting = useStore((s) => !!s.connecting[id]);
  const [password, setPassword] = useState("");
  const [remember, setRemember] = useState(false);
  const close = () => useStore.getState().openDialog(null);
  if (!registration) return null;

  return (
    <Modal
      title={t("Connect to {name}", { name: registration.name })}
      width="w-[24rem]"
      onCancel={close}
      onSubmit={() => void useStore.getState().connect(id, password, remember)}
      footer={
        <>
          <span className="flex-1" />
          <Button onClick={close}>{t("Cancel")}</Button>
          <Button primary submit disabled={connecting}>
            {connecting ? t("Connecting…") : t("Connect")}
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-2.5">
        <p className="text-muted">
          {registration.user}@{registration.host}:{registration.database}
        </p>
        <Field label={t("Password")}>
          <input className={inputClass} type="password" value={password} onChange={(e) => setPassword(e.target.value)} />
        </Field>
        <label className="flex items-center gap-2">
          <input
            type="checkbox"
            checked={remember}
            onChange={(e) => setRemember(e.target.checked)}
            className="accent-[var(--accent)]"
          />
          {t("Save password in the system keychain")}
        </label>
        {error && <p className="rounded border border-danger/40 px-2.5 py-1.5 text-danger">{error}</p>}
      </div>
    </Modal>
  );
}

function DeleteDialog({ id }: { id: string }) {
  const registration = useStore((s) => s.registrations.find((r) => r.id === id));
  const [error, setError] = useState<string | null>(null);
  const close = () => useStore.getState().openDialog(null);
  if (!registration) return null;

  const remove = async () => {
    try {
      await useStore.getState().deleteRegistration(id);
      close();
    } catch (e) {
      setError(errorText(e));
    }
  };

  return (
    <Modal
      title={t("Unregister Database")}
      width="w-[26rem]"
      onCancel={close}
      onSubmit={remove}
      footer={
        <>
          <span className="flex-1" />
          <Button onClick={close}>{t("Cancel")}</Button>
          <Button danger submit>
            {t("Unregister")}
          </Button>
        </>
      }
    >
      <p>{t("Remove {name} from the explorer?", { name: registration.name })}</p>
      <p className="mt-1.5 text-muted">
        {t("The database itself is not touched. Its saved password is deleted from the keychain.")}
      </p>
      {error && <p className="mt-3 rounded border border-danger/40 px-2.5 py-1.5 text-danger">{error}</p>}
    </Modal>
  );
}
