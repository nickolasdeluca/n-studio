// The code formatter's style: its settings on the left, what they do to a
// sample on the right. Save keeps it for this machine; Export writes it to
// a file for the rest of a team, and Import reads theirs.
import { open, save } from "@tauri-apps/plugin-dialog";
import { useEffect, useState } from "react";
import { api, errorText, type FormatCase, type FormatLists, type FormatStyle } from "../api";
import { formatStyle, saveFormatStyle } from "../format";
import { t } from "../i18n";
import { useStore } from "../store";
import { Button, Field, inputClass, Modal } from "./Modal";

const SAMPLE = `SELECT c.ID, c.NOME, (select count(*) from PEDIDO p where p.CLIENTE = c.ID and p.EMISSAO >= :desde) as PEDIDOS, coalesce(ci.NOME, '?') from CLIENTE c left join CIDADE ci on ci.ID = c.CIDADE and ci.UF = 'SC' where c.ATIVO = 1 and (c.NOME starting with 'A' or c.ID in (1,2,3)) order by c.NOME;

update PEDIDO set TOTAL = TOTAL * 1.1, ALTERADO = current_timestamp where ID = :id;

create procedure P_TOTAL (PEDIDO integer, TAXA numeric(9,2) = 0) returns (TOTAL numeric(18,2)) as declare variable V numeric(18,2); begin for select i.QTD * i.PRECO from PEDIDO_ITEM i where i.PEDIDO = :PEDIDO into :V do begin if (V > 0) then TOTAL = TOTAL + V; else TOTAL = 0; end suspend; end`;

const cases: [FormatCase, string][] = [
  ["upper", "UPPER CASE"],
  ["lower", "lower case"],
  ["keep", "As written"],
];
const lists: [FormatLists, string][] = [
  ["one_per_line", "One per line"],
  ["wrap", "Wrapped at the right margin"],
];
const filters = () => [{ name: t("Formatting style"), extensions: ["json"] }];

export function FormatterDialog() {
  const [style, setStyle] = useState<FormatStyle | null>(null);
  const [preview, setPreview] = useState("");
  const [note, setNote] = useState<{ text: string; error?: boolean } | null>(null);
  const close = () => useStore.getState().openDialog(null);

  useEffect(() => {
    void formatStyle().then(setStyle, (e) => setNote({ text: errorText(e), error: true }));
  }, []);

  useEffect(() => {
    if (!style) return;
    let live = true;
    const timer = setTimeout(() => {
      api.formatSql(SAMPLE, style).then(
        (text) => live && setPreview(text),
        (e) => live && setNote({ text: errorText(e), error: true }),
      );
    }, 80);
    return () => {
      live = false;
      clearTimeout(timer);
    };
  }, [style]);

  if (!style) return null;
  const set = (patch: Partial<FormatStyle>) => {
    setStyle({ ...style, ...patch });
    setNote(null);
  };
  const number = (key: "indent" | "rightMargin" | "inlineSubquery", min: number, max: number) => (
    <input
      type="number"
      className={`${inputClass} w-20`}
      min={min}
      max={max}
      value={style[key]}
      onChange={(e) => set({ [key]: Math.max(min, Math.min(max, Math.trunc(Number(e.target.value)) || min)) })}
    />
  );
  const choice = <T extends string>(value: T, options: [T, string][], change: (v: T) => void) => (
    <select className={inputClass} value={value} onChange={(e) => change(e.target.value as T)}>
      {options.map(([id, label]) => (
        <option key={id} value={id}>
          {t(label)}
        </option>
      ))}
    </select>
  );
  const check = (key: "breakConditions" | "breakJoinOn" | "spaceAfterComma" | "spaceInsideBrackets", label: string) => (
    <label className="flex items-center gap-2">
      <input type="checkbox" checked={style[key]} onChange={(e) => set({ [key]: e.target.checked })} />
      {t(label)}
    </label>
  );

  const submit = async () => {
    try {
      await saveFormatStyle(style);
      close();
    } catch (e) {
      setNote({ text: errorText(e), error: true });
    }
  };
  const exportStyle = async () => {
    try {
      const path = await save({ title: t("Export Formatting Style"), defaultPath: "sql-style.json", filters: filters() });
      if (!path) return;
      await api.formatStyleExport(path, style);
      setNote({ text: t("Exported to {path}", { path }) });
    } catch (e) {
      setNote({ text: errorText(e), error: true });
    }
  };
  const importStyle = async () => {
    try {
      const path = await open({ title: t("Import Formatting Style"), multiple: false, filters: filters() });
      if (typeof path !== "string") return;
      setStyle(await api.formatStyleImport(path));
      setNote({ text: t("Imported: Save makes it the style in use.") });
    } catch (e) {
      setNote({ text: errorText(e), error: true });
    }
  };
  const defaults = async () => {
    try {
      // The defaults are the backend's: a style file with nothing in it
      setStyle(await api.formatStyleDefault());
      setNote(null);
    } catch (e) {
      setNote({ text: errorText(e), error: true });
    }
  };

  return (
    <Modal
      title={t("Code Formatter")}
      width="w-[64rem]"
      onCancel={close}
      onSubmit={() => void submit()}
      footer={
        <>
          <Button onClick={() => void importStyle()}>{t("Import…")}</Button>
          <Button onClick={() => void exportStyle()}>{t("Export…")}</Button>
          <Button onClick={() => void defaults()}>{t("Defaults")}</Button>
          <span className={`min-w-0 flex-1 truncate px-1 ${note?.error ? "text-danger" : "text-muted"}`} title={note?.text}>
            {note?.text}
          </span>
          <Button onClick={close}>{t("Cancel")}</Button>
          <Button primary submit>
            {t("Save")}
          </Button>
        </>
      }
    >
      <div className="flex gap-4">
        <div className="flex w-64 shrink-0 flex-col gap-3">
          <Field label={t("Keywords")}>{choice(style.keywords, cases, (keywords) => set({ keywords }))}</Field>
          <Field label={t("Identifiers")} hint={t("Names in double quotes are never changed.")}>
            {choice(style.identifiers, cases, (identifiers) => set({ identifiers }))}
          </Field>
          <div className="flex gap-3">
            <Field label={t("Indentation")}>{number("indent", 1, 8)}</Field>
            <Field label={t("Right margin")}>{number("rightMargin", 40, 400)}</Field>
          </div>
          <Field label={t("SELECT columns, SET assignments")}>
            {choice(style.selectList, lists, (selectList) => set({ selectList }))}
          </Field>
          <Field label={t("Table columns, routine parameters")}>
            {choice(style.definitions, lists, (definitions) => set({ definitions }))}
          </Field>
          <Field label={t("A subquery stays on its line up to")} hint={t("Characters. 0: always on lines of its own.")}>
            {number("inlineSubquery", 0, 400)}
          </Field>
          <div className="flex flex-col gap-1.5">
            {check("breakConditions", "AND and OR start a line")}
            {check("breakJoinOn", "A join's ON starts a line")}
            {check("spaceAfterComma", "A space after a comma")}
            {check("spaceInsideBrackets", "Spaces inside brackets")}
          </div>
        </div>
        <pre className="scroll-thin h-[30rem] min-w-0 flex-1 overflow-auto rounded border border-line bg-bg p-2 font-mono text-[12px] select-text">
          {preview}
        </pre>
      </div>
    </Modal>
  );
}
