import { useState } from "react";
import { api, errorText, serverSummary, type Registration } from "../api";
import { t } from "../i18n";
import { useStore } from "../store";
import { Button, Field, inputClass, Modal } from "./Modal";

const blank: Registration = {
  engine: "firebird",
  id: "",
  name: "",
  savePassword: true,
  host: "localhost",
  port: 3050,
  database: "",
  user: "SYSDBA",
  role: "",
  // Most of the legacy databases this is for are WIN1252
  charset: "WIN1252",
  dialect: "3",
  auth: "srp",
};

type Test = { state: "idle" } | { state: "running" } | { state: "ok"; text: string } | { state: "failed"; text: string };

/** Register a database, or edit a registration */
export function RegistrationDialog({ registration }: { registration: Registration | null }) {
  const charsets = useStore((s) => s.charsets);
  const editing = !!registration;
  const [r, setR] = useState<Registration>(registration ?? blank);
  const [password, setPassword] = useState("");
  // Until the user edits the name, it follows the database file name
  const [nameTouched, setNameTouched] = useState(editing);
  const [test, setTest] = useState<Test>({ state: "idle" });
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  const close = () => useStore.getState().openDialog(null);
  const update = (patch: Partial<Registration>) => {
    setR((r) => ({ ...r, ...patch }));
    setTest({ state: "idle" });
    setError(null);
  };
  // Editing leaves the saved password alone unless a new one is typed
  const passwordArg = password || (editing ? null : "");

  const runTest = async () => {
    setTest({ state: "running" });
    try {
      const server = await api.testConnection(r, passwordArg);
      setTest({ state: "ok", text: t("Connected: {summary}", { summary: serverSummary(server) }) });
    } catch (e) {
      setTest({ state: "failed", text: errorText(e) });
    }
  };

  const save = async () => {
    setSaving(true);
    try {
      // An empty field saves nothing: the password is asked on connect
      const saved = await useStore.getState().saveRegistration(r, r.savePassword && password ? password : null);
      close();
      useStore.getState().select({ id: saved.id });
    } catch (e) {
      setError(errorText(e));
    } finally {
      setSaving(false);
    }
  };

  return (
    <Modal
      title={editing ? t("Database Registration — {name}", { name: registration.name }) : t("Register Database")}
      width="w-[34rem]"
      onCancel={close}
      onSubmit={save}
      footer={
        <>
          <Button onClick={runTest} disabled={test.state === "running"}>
            {test.state === "running" ? t("Testing…") : t("Test Connection")}
          </Button>
          <span className="flex-1" />
          <Button onClick={close}>{t("Cancel")}</Button>
          <Button primary submit disabled={saving}>
            {editing ? t("Save") : t("Register")}
          </Button>
        </>
      }
    >
      <div className="grid grid-cols-6 gap-x-3 gap-y-2.5">
        <Field label={t("Server")} className="col-span-4">
          <input
            className={inputClass}
            value={r.host}
            onChange={(e) => update({ host: e.target.value })}
            spellCheck={false}
          />
        </Field>
        <Field label={t("Port")} className="col-span-2">
          <input
            className={inputClass}
            type="number"
            min={1}
            max={65535}
            value={r.port}
            onChange={(e) => update({ port: Number(e.target.value) || 3050 })}
          />
        </Field>
        <Field
          label={t("Database file or alias")}
          hint={t("As the server sees it: a path on the server, or an alias from its databases.conf.")}
          className="col-span-6"
        >
          <input
            className={`${inputClass} font-mono text-[12px]`}
            value={r.database}
            placeholder="/var/lib/firebird/data/employee.fdb"
            spellCheck={false}
            data-autofocus={editing ? undefined : true}
            onChange={(e) => {
              const database = e.target.value;
              update(nameTouched ? { database } : { database, name: aliasFrom(database) });
            }}
          />
        </Field>
        <Field label={t("Name in the explorer")} className="col-span-6">
          <input
            className={inputClass}
            value={r.name}
            data-autofocus={editing ? true : undefined}
            onChange={(e) => {
              setNameTouched(true);
              update({ name: e.target.value });
            }}
          />
        </Field>

        <div className="col-span-6 mt-1 border-t border-line" />

        <Field label={t("User")} className="col-span-3">
          <input className={inputClass} value={r.user} onChange={(e) => update({ user: e.target.value })} spellCheck={false} />
        </Field>
        <Field label={t("Role")} className="col-span-3">
          <input
            className={inputClass}
            value={r.role}
            placeholder={t("none")}
            onChange={(e) => update({ role: e.target.value })}
            spellCheck={false}
          />
        </Field>
        <Field label={t("Password")} className="col-span-3">
          <input
            className={inputClass}
            type="password"
            value={password}
            placeholder={editing && registration.savePassword ? t("unchanged") : ""}
            onChange={(e) => {
              setPassword(e.target.value);
              setTest({ state: "idle" });
            }}
          />
        </Field>
        <label className="col-span-3 flex items-end gap-2 pb-1.5">
          <input
            type="checkbox"
            checked={r.savePassword}
            onChange={(e) => update({ savePassword: e.target.checked })}
            className="accent-[var(--accent)]"
          />
          <span>{t("Save password in the system keychain")}</span>
        </label>

        <div className="col-span-6 mt-1 border-t border-line" />

        <Field label={t("Character set")} className="col-span-2">
          <select className={inputClass} value={r.charset} onChange={(e) => update({ charset: e.target.value })}>
            {charsets.map((c) => (
              <option key={c}>{c}</option>
            ))}
          </select>
        </Field>
        <Field label={t("SQL dialect")} className="col-span-2">
          <select
            className={inputClass}
            value={r.dialect}
            onChange={(e) => update({ dialect: e.target.value as Registration["dialect"] })}
          >
            <option value="3">3</option>
            <option value="1">1</option>
          </select>
        </Field>
        <Field label={t("Authentication")} className="col-span-2">
          <select
            className={inputClass}
            value={r.auth}
            onChange={(e) => update({ auth: e.target.value as Registration["auth"] })}
          >
            <option value="srp">{t("SRP (Firebird 3+)")}</option>
            <option value="legacy">{t("Legacy")}</option>
          </select>
        </Field>
      </div>

      {(test.state === "ok" || test.state === "failed") && (
        <p className={`mt-3 rounded border px-2.5 py-1.5 ${test.state === "ok" ? "border-ok/40 text-ok" : "border-danger/40 text-danger"}`}>
          {test.text}
        </p>
      )}
      {error && <p className="mt-3 rounded border border-danger/40 px-2.5 py-1.5 text-danger">{error}</p>}
    </Modal>
  );
}

/** "C:\data\Sales.FDB" → "SALES"; an alias stays as it is */
function aliasFrom(database: string): string {
  const file = database.trim().split(/[\\/:]/).pop() ?? "";
  return file.replace(/\.(fdb|gdb|ib)$/i, "").toUpperCase();
}
