// Import from IBExpert: its registered databases and its code formatter's
// options. IBExpert keeps them in its User Database, a Firebird database,
// so the dialog asks where that is, shows what it has, and brings over
// what is ticked.
import { useState } from "react";
import { api, errorText, type FormatStyle, type IbexpertFound, type Registration } from "../api";
import { saveFormatStyle } from "../format";
import { t } from "../i18n";
import { useStore } from "../store";
import { Button, Field, inputClass, Modal } from "./Modal";

const blank: Registration = {
  engine: "firebird",
  id: "",
  name: "IBExpert",
  savePassword: false,
  host: "localhost",
  port: 3050,
  database: "",
  user: "SYSDBA",
  role: "",
  charset: "WIN1252",
  dialect: "3",
  auth: "srp",
};

const caseNames = { upper: "UPPER CASE", lower: "lower case", keep: "as written" } as const;

/** A style in a line: the settings IBExpert's options decide */
function styleSummary(s: FormatStyle): string {
  return [
    t("keywords {case}", { case: t(caseNames[s.keywords]) }),
    t("identifiers {case}", { case: t(caseNames[s.identifiers]) }),
    t("indentation {n}", { n: s.indent }),
    t("right margin {n}", { n: s.rightMargin }),
    s.selectList === "one_per_line" ? t("one column per line") : t("columns wrapped"),
    s.breakConditions ? t("AND and OR start a line") : t("AND and OR stay on the line"),
  ].join(", ");
}

export function IbexpertDialog() {
  const [source, setSource] = useState<Registration>(blank);
  const [password, setPassword] = useState("");
  const [found, setFound] = useState<IbexpertFound | null>(null);
  // The databases ticked, by their place in the list
  const [chosen, setChosen] = useState<Set<number>>(new Set());
  const [withStyle, setWithStyle] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const close = () => useStore.getState().openDialog(null);
  const update = (patch: Partial<Registration>) => {
    setSource((s) => ({ ...s, ...patch }));
    // Another database: what was read is not its
    setFound(null);
    setError(null);
  };

  const read = async () => {
    setBusy(true);
    setError(null);
    try {
      const result = await api.ibexpertScan(source, password);
      setFound(result);
      setChosen(new Set(result.databases.flatMap((d, i) => (d.registered ? [] : [i]))));
      setWithStyle(!!result.style);
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(false);
    }
  };

  const databases = found ? found.databases.filter((_, i) => chosen.has(i)) : [];
  const style = withStyle ? (found?.style ?? null) : null;
  const nothing = databases.length === 0 && !style;

  const bringOver = async () => {
    setBusy(true);
    setError(null);
    let saved = 0;
    try {
      for (const d of databases) {
        await useStore.getState().saveRegistration(d.registration, null);
        saved += 1;
      }
      if (style) await saveFormatStyle(style);
      close();
      useStore.getState().setNotice(
        [
          saved > 0 && (saved === 1 ? t("1 database registered") : t("{n} databases registered", { n: saved })),
          style && t("the formatting style is IBExpert's"),
        ]
          .filter(Boolean)
          .join(", ") + ".",
      );
    } catch (e) {
      // What was saved before it failed is in the explorer already
      setError(errorText(e));
      setBusy(false);
    }
  };

  const toggle = (i: number) => {
    const next = new Set(chosen);
    if (!next.delete(i)) next.add(i);
    setChosen(next);
  };

  return (
    <Modal
      title={t("Import from IBExpert")}
      width="w-[40rem]"
      onCancel={close}
      onSubmit={() => void (found ? !nothing && bringOver() : read())}
      footer={
        <>
          <span className="min-w-0 flex-1 text-muted">
            {found && t("Passwords are not imported: each is asked for on the first connect.")}
          </span>
          <Button onClick={close}>{t("Cancel")}</Button>
          {found ? (
            <Button primary submit disabled={busy || nothing}>
              {t("Import")}
            </Button>
          ) : (
            <Button primary submit disabled={busy || !source.database.trim()}>
              {busy ? t("Reading…") : t("Read")}
            </Button>
          )}
        </>
      }
    >
      <p className="mb-2.5 text-muted">
        {t("IBExpert keeps its settings in its User Database, a Firebird database (Options, Environment Options, User Database). Say where it is: it is only read.")}
      </p>
      <div className="grid grid-cols-6 gap-x-3 gap-y-2.5">
        <Field label={t("Server")} className="col-span-4">
          <input className={inputClass} value={source.host} onChange={(e) => update({ host: e.target.value })} spellCheck={false} />
        </Field>
        <Field label={t("Port")} className="col-span-2">
          <input
            className={inputClass}
            type="number"
            min={1}
            max={65535}
            value={source.port}
            onChange={(e) => update({ port: Number(e.target.value) || 3050 })}
          />
        </Field>
        <Field label={t("User Database file or alias")} className="col-span-6">
          <input
            className={`${inputClass} font-mono text-[12px]`}
            value={source.database}
            placeholder="C:\IBExpert\ibexpert.fdb"
            spellCheck={false}
            data-autofocus
            onChange={(e) => update({ database: e.target.value })}
          />
        </Field>
        <Field label={t("User")} className="col-span-2">
          <input className={inputClass} value={source.user} onChange={(e) => update({ user: e.target.value })} spellCheck={false} />
        </Field>
        <Field label={t("Password")} className="col-span-2">
          <input
            className={inputClass}
            type="password"
            value={password}
            onChange={(e) => {
              setPassword(e.target.value);
              setFound(null);
              setError(null);
            }}
          />
        </Field>
        <Field label={t("Authentication")} className="col-span-2">
          <select className={inputClass} value={source.auth} onChange={(e) => update({ auth: e.target.value as Registration["auth"] })}>
            <option value="srp">{t("SRP (Firebird 3+)")}</option>
            <option value="legacy">{t("Legacy")}</option>
          </select>
        </Field>
      </div>

      {found && (
        <div className="mt-3 flex flex-col gap-2.5 border-t border-line pt-3">
          {found.databases.length === 0 ? (
            <p className="text-muted">{t("No databases are registered in it.")}</p>
          ) : (
            <ul className="scroll-thin max-h-64 overflow-auto rounded border border-line bg-bg">
              {found.databases.map((d, i) => (
                <li key={i} className="border-b border-line last:border-b-0">
                  <label className="flex items-start gap-2 px-2.5 py-1.5">
                    <input
                      type="checkbox"
                      checked={chosen.has(i)}
                      onChange={() => toggle(i)}
                      className="mt-0.5 accent-[var(--accent)]"
                    />
                    <span className="min-w-0 flex-1">
                      <span className="flex items-baseline gap-2">
                        <span className="font-medium">{d.registration.name}</span>
                        {d.folder && <span className="truncate text-[11.5px] text-faint">{d.folder}</span>}
                        {d.registered && <span className="ml-auto shrink-0 text-[11.5px] text-muted">{t("already registered")}</span>}
                      </span>
                      <span className="block truncate font-mono text-[11.5px] text-muted">
                        {d.registration.user}@{d.registration.host}
                        {d.registration.port === 3050 ? "" : `/${d.registration.port}`}:{d.registration.database}
                      </span>
                      {d.charsetReplaced !== null && (
                        <span className="block text-[11.5px] text-faint">
                          {t("Character set {theirs} becomes {ours}: change it in the registration if the database is not that.", {
                            theirs: d.charsetReplaced || t("(none)"),
                            ours: d.registration.charset,
                          })}
                        </span>
                      )}
                    </span>
                  </label>
                </li>
              ))}
            </ul>
          )}
          {found.style ? (
            <label className="flex items-start gap-2">
              <input
                type="checkbox"
                checked={withStyle}
                onChange={(e) => setWithStyle(e.target.checked)}
                className="mt-0.5 accent-[var(--accent)]"
              />
              <span>
                {t("The code formatter's style, in place of the one in use")}
                <span className="block text-[11.5px] text-muted">{styleSummary(found.style)}</span>
              </span>
            </label>
          ) : (
            <p className="text-muted">{t("It has no code formatter options.")}</p>
          )}
          <p className="text-[11.5px] text-faint">
            {t("Imported registrations use SQL dialect 3 and SRP authentication: edit the ones on Firebird 2.5 or older, or in dialect 1.")}
          </p>
        </div>
      )}
      {error && <p className="mt-3 rounded border border-danger/40 px-2.5 py-1.5 text-danger">{error}</p>}
    </Modal>
  );
}
