import { useState } from "react";
import { errorText } from "../api";
import { panelTitle } from "../dock";
import { t } from "../i18n";
import type { ParamRequest, ParamValue } from "../params";
import { activeTabs, resolve, type PendingAction } from "../pending";
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
    case "pending":
      return <PendingDialog tabs={dialog.tabs} action={dialog.action} />;
    case "params":
      return <ParamsDialog request={dialog.request} />;
  }
}

/** What a value of each kind looks like, as the server reads it from text */
const hints: Partial<Record<string, string>> = {
  date: "YYYY-MM-DD",
  time: "HH:MM:SS",
  timestamp: "YYYY-MM-DD HH:MM:SS",
  boolean: "TRUE / FALSE",
};

function ParamsDialog({ request }: { request: ParamRequest }) {
  const [values, setValues] = useState<Record<string, ParamValue>>(() =>
    Object.fromEntries(request.fields.map((f) => [f.key, request.initial[f.key] ?? { value: "", isNull: false }])),
  );
  const set = (key: string, v: Partial<ParamValue>) => setValues((vs) => ({ ...vs, [key]: { ...vs[key], ...v } }));
  const finish = (result: Record<string, ParamValue> | null) => {
    // Closed first, so the store does not cancel the request it replaces
    useStore.setState({ dialog: null });
    request.done(result);
  };

  return (
    <Modal
      title={t("Parameters")}
      width="w-[30rem]"
      onCancel={() => finish(null)}
      onSubmit={() => finish(values)}
      footer={
        <>
          <span className="flex-1 text-muted">{t("Values are sent as text; the server converts them.")}</span>
          <Button onClick={() => finish(null)}>{t("Cancel")}</Button>
          <Button primary submit>
            {t("Execute")}
          </Button>
        </>
      }
    >
      <div className="grid grid-cols-[auto_1fr_auto] items-center gap-x-2.5 gap-y-2">
        {request.fields.map((f, i) => {
          const v = values[f.key];
          return (
            <div key={f.key} className="contents">
              <label htmlFor={`param-${i}`} className="font-mono" title={f.type.typeName}>
                {f.key.startsWith("?") ? f.key : `:${f.key}`}
                <span className="ml-2 font-sans text-[11px] text-faint">{f.type.typeName}</span>
              </label>
              <input
                id={`param-${i}`}
                className={`${inputClass} font-mono`}
                value={v.isNull ? "" : v.value}
                placeholder={v.isNull ? "NULL" : hints[f.type.kind]}
                disabled={v.isNull}
                data-autofocus={i === 0 ? true : undefined}
                onChange={(e) => set(f.key, { value: e.target.value })}
              />
              <label className="flex items-center gap-1.5 text-muted">
                <input
                  type="checkbox"
                  checked={v.isNull}
                  onChange={(e) => set(f.key, { isNull: e.target.checked })}
                  className="accent-[var(--accent)]"
                />
                NULL
              </label>
            </div>
          );
        })}
      </div>
    </Modal>
  );
}

function PendingDialog({ tabs, action }: { tabs: string[]; action: PendingAction }) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const close = () => useStore.getState().openDialog(null);
  const end = async (how: "commit" | "rollback") => {
    setBusy(true);
    setError(null);
    const failed = await resolve(action, how);
    setBusy(false);
    if (failed) setError(failed);
    else close();
  };
  const what = {
    close: t("Close the editor"),
    disconnect: t("Disconnect"),
    quit: t("Quit N Studio"),
  }[action.kind];

  return (
    <Modal
      title={t("Active Transactions")}
      width="w-[28rem]"
      onCancel={close}
      onSubmit={() => void end("commit")}
      footer={
        <>
          <span className="flex-1" />
          <Button onClick={close} disabled={busy}>
            {t("Cancel")}
          </Button>
          <Button onClick={() => void end("rollback")} disabled={busy}>
            {t("Roll Back")}
          </Button>
          <Button primary submit disabled={busy}>
            {t("Commit")}
          </Button>
        </>
      }
    >
      <p>
        {tabs.length === 1
          ? t("This editor has an active transaction:")
          : t("These editors have active transactions:")}
      </p>
      <ul className="mt-1.5 list-disc pl-5 text-muted">
        {tabs.map((tab) => (
          <li key={tab}>{panelTitle(tab)}</li>
        ))}
      </ul>
      <p className="mt-2.5">{t("{what}: commit or roll back first?", { what })}</p>
      {error && <p className="mt-3 rounded border border-danger/40 px-2.5 py-1.5 whitespace-pre-wrap text-danger">{error}</p>}
    </Modal>
  );
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
      {activeTabs((r) => r === id).length > 0 && (
        <p className="mt-1.5 text-danger">{t("Its editors' active transactions are rolled back.")}</p>
      )}
      {error && <p className="mt-3 rounded border border-danger/40 px-2.5 py-1.5 text-danger">{error}</p>}
    </Modal>
  );
}
