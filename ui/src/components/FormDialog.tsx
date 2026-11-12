// What an action asks before it can be done: the fields the backend named,
// drawn by their kind. Filled, it becomes a script for the compile dialog;
// nothing here knows what the fields mean.
import { useId, useState } from "react";
import { errorText, type FormField } from "../api";
import { t } from "../i18n";
import { useStore, type Dialog } from "../store";
import { Button, Field, inputClass, Modal } from "./Modal";

type FormRequest = Extract<Dialog, { type: "form" }>;

export function FormDialog({ dialog }: { dialog: FormRequest }) {
  const { form } = dialog;
  const [values, setValues] = useState<Record<string, string>>(
    () => dialog.values ?? Object.fromEntries(form.fields.map((f) => [f.id, f.value])),
  );
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const close = () => useStore.getState().openDialog(null);
  const missing = form.fields.some((f) => f.required && !(values[f.id] ?? "").trim());

  const submit = async () => {
    if (busy || missing) return;
    setBusy(true);
    setError(null);
    try {
      // Cancelled, what it leads to comes back here with what was typed
      await dialog.submit(values, { ...dialog, values });
    } catch (e) {
      setError(errorText(e));
      setBusy(false);
    }
  };

  return (
    <Modal
      title={dialog.title}
      width="w-[32rem]"
      onCancel={close}
      onSubmit={() => void submit()}
      footer={
        <>
          <span className="flex-1 text-muted">{dialog.note ?? t("The script is shown before anything runs.")}</span>
          <Button onClick={close}>{t("Cancel")}</Button>
          <Button primary submit disabled={busy || missing}>
            {dialog.verb ?? t("Continue…")}
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-2.5">
        {form.fields.map((f) => (
          <Input key={f.id} field={f} value={values[f.id] ?? ""} onChange={(v) => setValues((vs) => ({ ...vs, [f.id]: v }))} />
        ))}
        {error && (
          <p className="rounded border border-danger/40 px-2.5 py-1.5 whitespace-pre-wrap text-danger select-text" role="alert">
            {error}
          </p>
        )}
      </div>
    </Modal>
  );
}

function Input({ field, value, onChange }: { field: FormField; value: string; onChange(value: string): void }) {
  const list = useId();
  const hint = field.hint ? t(field.hint) : undefined;
  if (field.kind === "check") {
    return (
      <div className="flex flex-col gap-1">
        <label className="flex items-center gap-2">
          <input
            type="checkbox"
            checked={value === "true"}
            onChange={(e) => onChange(e.target.checked ? "true" : "false")}
            className="accent-[var(--accent)]"
          />
          {t(field.label)}
        </label>
        {hint && <span className="text-[11.5px] text-faint">{hint}</span>}
      </div>
    );
  }
  const label = field.required ? `${t(field.label)} *` : t(field.label);
  let input;
  switch (field.kind) {
    case "select":
      input = (
        <select className={inputClass} value={value} onChange={(e) => onChange(e.target.value)}>
          {field.options.map((option) => (
            <option key={option} value={option}>
              {option}
            </option>
          ))}
        </select>
      );
      break;
    case "code":
      // Enter is a new line here: an expression may take several
      input = (
        <textarea
          className={`${inputClass} scroll-thin h-auto min-h-7 resize-y py-1 font-mono text-[12px]`}
          rows={Math.min(6, Math.max(1, value.split("\n").length))}
          value={value}
          spellCheck={false}
          onChange={(e) => onChange(e.target.value)}
        />
      );
      break;
    case "combo":
      input = (
        <>
          <input
            className={inputClass}
            list={list}
            value={value}
            spellCheck={false}
            autoComplete="off"
            onChange={(e) => onChange(e.target.value)}
          />
          <datalist id={list}>
            {field.options.map((option) => (
              <option key={option} value={option} />
            ))}
          </datalist>
        </>
      );
      break;
    default:
      input = (
        <input
          className={`${inputClass} ${field.kind === "number" ? "w-40 tabular-nums" : ""}`}
          type={field.kind === "password" ? "password" : undefined}
          inputMode={field.kind === "number" ? "numeric" : undefined}
          value={value}
          spellCheck={false}
          autoComplete="off"
          onChange={(e) => onChange(e.target.value)}
        />
      );
  }
  return (
    <Field label={label} hint={hint}>
      {input}
    </Field>
  );
}
