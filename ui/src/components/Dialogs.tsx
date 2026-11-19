import { save } from "@tauri-apps/plugin-dialog";
import { useState } from "react";
import { api, errorText, type ExportOptions } from "../api";
import { pageOf, useEditors } from "../editors";
import { panelTitle } from "../dock";
import { t } from "../i18n";
import type { ParamRequest, ParamValue } from "../params";
import { addConnect } from "../executive";
import { activeTabs, request, resolve, type PendingAction } from "../pending";
import { pageName, saveEditor } from "../scripts";
import { useStore } from "../store";
import { CompileDialog, DiscardDialog } from "./CompileDialog";
import { FormDialog } from "./FormDialog";
import { FormatterDialog } from "./FormatterDialog";
import { IbexpertDialog } from "./IbexpertDialog";
import { Button, Field, inputClass, Modal } from "./Modal";
import { RegistrationDialog } from "./RegistrationDialog";

/** Whichever dialog the store has open */
export function Dialogs() {
  const dialog = useStore((s) => s.dialog);
  if (!dialog) return null;
  switch (dialog.type) {
    case "registration":
      return <RegistrationDialog registration={dialog.registration} cloneOf={dialog.cloneOf} />;
    case "password":
      return <PasswordDialog id={dialog.id} error={dialog.error} />;
    case "scriptPassword":
      return <ScriptPasswordDialog {...dialog} />;
    case "connectStatement":
      return <ConnectStatementDialog panel={dialog.panel} />;
    case "delete":
      return <DeleteDialog id={dialog.id} />;
    case "pending":
      return <PendingDialog tabs={dialog.tabs} action={dialog.action} />;
    case "unsaved":
      return <UnsavedDialog tab={dialog.tab} page={dialog.page} answered={dialog.answered} action={dialog.action} />;
    case "params":
      return <ParamsDialog request={dialog.request} />;
    case "export":
      return <ExportDialog tab={dialog.tab} />;
    case "formatter":
      return <FormatterDialog />;
    case "ibexpert":
      return <IbexpertDialog />;
    case "compile":
      // Another change is another dialog: its outcome is not the last one's
      return <CompileDialog key={dialog.script} dialog={dialog} />;
    case "form":
      return <FormDialog dialog={dialog} />;
    case "discard":
      return <DiscardDialog viewers={dialog.viewers} action={dialog.action} />;
  }
}

const formats = [
  { id: "csv", label: "CSV, comma separated", delimiter: ",", ext: "csv" },
  { id: "csv-semicolon", label: "CSV, semicolon separated", delimiter: ";", ext: "csv" },
  { id: "tsv", label: "TSV, tab separated", delimiter: "\t", ext: "tsv" },
] as const;

/** The last export's choices, for the next one */
let lastExport: { format: (typeof formats)[number]["id"]; header: boolean; bom: boolean } = {
  format: "csv",
  header: true,
  bom: true,
};

function ExportDialog({ tab }: { tab: string }) {
  const [choice, setChoice] = useState(lastExport);
  const [error, setError] = useState<string | null>(null);
  const close = () => useStore.getState().openDialog(null);
  const format = formats.find((f) => f.id === choice.format) ?? formats[0];

  const exportRows = async () => {
    setError(null);
    let path: string | null;
    try {
      path = await save({
        title: t("Export Rows"),
        defaultPath: `${panelTitle(tab).replace(/[\\/:*?"<>|]/g, "_")}.${format.ext}`,
        filters: [{ name: format.ext.toUpperCase(), extensions: [format.ext] }],
      });
    } catch (e) {
      setError(errorText(e));
      return;
    }
    if (!path) return;
    lastExport = choice;
    close();
    const options: ExportOptions = { delimiter: format.delimiter, header: choice.header, bom: choice.bom };
    await useEditors.getState().exportRows(tab, path, options);
  };

  return (
    <Modal
      title={t("Export Rows")}
      width="w-[26rem]"
      onCancel={close}
      onSubmit={() => void exportRows()}
      footer={
        <>
          <span className="flex-1" />
          <Button onClick={close}>{t("Cancel")}</Button>
          <Button primary submit>
            {t("Export…")}
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-2.5">
        <Field label={t("Format")}>
          <select
            className={inputClass}
            value={choice.format}
            onChange={(e) => setChoice({ ...choice, format: e.target.value as typeof choice.format })}
          >
            {formats.map((f) => (
              <option key={f.id} value={f.id}>
                {t(f.label)}
              </option>
            ))}
          </select>
        </Field>
        <label className="flex items-center gap-2">
          <input
            type="checkbox"
            checked={choice.header}
            onChange={(e) => setChoice({ ...choice, header: e.target.checked })}
            className="accent-[var(--accent)]"
          />
          {t("Column names on the first line")}
        </label>
        <label className="flex items-center gap-2">
          <input
            type="checkbox"
            checked={choice.bom}
            onChange={(e) => setChoice({ ...choice, bom: e.target.checked })}
            className="accent-[var(--accent)]"
          />
          {t("UTF-8 byte order mark (for Excel)")}
        </label>
        <p className="text-muted">
          {t("Every row is exported, in the grid's order: the rows not fetched yet are fetched first. Blobs are read in full while the transaction lasts.")}
        </p>
        {error && <p className="rounded border border-danger/40 px-2.5 py-1.5 text-danger">{error}</p>}
      </div>
    </Modal>
  );
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
    close: action.kind === "close" && action.tab.startsWith("adm-exe-") ? t("Close the Script Executive") : t("Close the editor"),
    closePage: t("Close the page"),
    disconnect: t("Disconnect"),
    quit: t("Quit N Studio"),
    update: t("Update and restart"),
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
        {tabs.some((tab) => tab.startsWith("adm-exe-"))
          ? t("A script is running, or left a transaction active, in:")
          : tabs.length === 1
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

/** Writes a registered database's SET NAMES and CONNECT at the top of a
 *  Script Executive's script. Without its password unless asked: the run
 *  finds it in the keychain, and a script file is no place to keep one. */
function ConnectStatementDialog({ panel }: { panel: string }) {
  const registrations = useStore((s) => s.registrations);
  const [chosen, setChosen] = useState(registrations[0]?.id ?? "");
  const [withPassword, setWithPassword] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const close = () => useStore.getState().openDialog(null);
  const add = async () => {
    if (!chosen) return;
    setBusy(true);
    setError(null);
    const failed = await addConnect(panel, chosen, withPassword);
    setBusy(false);
    if (failed) setError(failed);
    else close();
  };
  return (
    <Modal
      title={t("Add CONNECT Statement")}
      width="w-[30rem]"
      onCancel={close}
      onSubmit={() => void add()}
      footer={
        <>
          <span className="flex-1" />
          <Button onClick={close} disabled={busy}>
            {t("Cancel")}
          </Button>
          <Button primary submit disabled={busy || !chosen}>
            {t("Add")}
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-2.5">
        <p>{t("The script will start with SET NAMES and CONNECT for:")}</p>
        {registrations.length === 0 ? (
          <p className="text-muted">{t("No database is registered.")}</p>
        ) : (
          <div className="max-h-56 overflow-auto rounded border border-line">
            {registrations.map((r) => (
              <label key={r.id} className={`flex cursor-pointer items-center gap-2 px-2.5 py-1.5 hover:bg-hover ${chosen === r.id ? "bg-hover" : ""}`}>
                <input type="radio" className="accent-[var(--accent)]" checked={chosen === r.id} onChange={() => setChosen(r.id)} />
                <span className="flex min-w-0 flex-col">
                  <span className="truncate">{r.name}</span>
                  <span className="truncate text-[11.5px] text-muted">
                    {r.user}@{r.host}/{r.port}:{r.database}
                  </span>
                </span>
              </label>
            ))}
          </div>
        )}
        <label className="flex items-start gap-2">
          <input
            type="checkbox"
            className="mt-0.5 accent-[var(--accent)]"
            checked={withPassword}
            onChange={(e) => setWithPassword(e.target.checked)}
          />
          <span>
            {t("Write the password in the script")}
            <span className="block text-muted">
              {withPassword
                ? t("The script, and any file it is saved to, will hold the password in plain text.")
                : t("Without it, the run takes the password from the keychain, or asks.")}
            </span>
          </span>
        </label>
        {error && <p className="rounded border border-danger/40 px-2.5 py-1.5 text-danger">{error}</p>}
      </div>
    </Modal>
  );
}

/** A Script Executive's run waits for the password of a database its
 *  script connects to. Never kept: the next run asks again, unless the
 *  database is registered with a saved password. */
function ScriptPasswordDialog({ run, database, user, title }: { run: number; database: string; user: string; title: string }) {
  const [password, setPassword] = useState("");
  const answer = (given: string | null) => {
    void api.executivePassword(run, given);
    useStore.getState().openDialog(null);
  };
  return (
    <Modal
      title={t("Password for {database}", { database })}
      width="w-[24rem]"
      onCancel={() => answer(null)}
      onSubmit={() => answer(password)}
      footer={
        <>
          <span className="flex-1" />
          <Button onClick={() => answer(null)}>{t("Cancel")}</Button>
          <Button primary submit>
            {t("Connect")}
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-2.5">
        <p className="text-muted">{t("{title} connects as {user}.", { title, user })}</p>
        <Field label={t("Password")}>
          <input className={inputClass} type="password" autoFocus value={password} onChange={(e) => setPassword(e.target.value)} />
        </Field>
      </div>
    </Modal>
  );
}

function UnsavedDialog(props: { tab: string; page: string; answered: string[]; action: PendingAction }) {
  const { tab, page, answered, action } = props;
  const [busy, setBusy] = useState(false);
  const editor = useEditors((s) => s.tabs[tab]);
  const close = () => useStore.getState().openDialog(null);
  // What comes next may ask something of its own: another page's file,
  // the transaction's end
  const go = () => {
    close();
    request(action, [...answered, page]);
  };
  const saveFirst = async () => {
    setBusy(true);
    const saved = await saveEditor(tab, false, page);
    setBusy(false);
    // Not saved is said in the status bar; the editor stays open
    if (saved) go();
    else close();
  };

  return (
    <Modal
      title={t("Unsaved Changes")}
      width="w-[26rem]"
      onCancel={close}
      onSubmit={() => void saveFirst()}
      footer={
        <>
          <span className="flex-1" />
          <Button onClick={close} disabled={busy}>
            {t("Cancel")}
          </Button>
          <Button onClick={go} disabled={busy}>
            {t("Don't Save")}
          </Button>
          <Button primary submit disabled={busy}>
            {t("Save")}
          </Button>
        </>
      }
    >
      <p>{t("{file} has changes that were not saved.", { file: editor && pageOf(editor, page)?.file ? pageName(pageOf(editor, page)!) : panelTitle(tab) })}</p>
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
