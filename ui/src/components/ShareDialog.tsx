// Giving the agent rows of a grid: the user's act, and only after being
// told where they go.
import { useState } from "react";
import { api, errorText, SHARE_ROWS } from "../api";
import { editorLabel, share } from "../agent";
import { useEditors } from "../editors";
import { t } from "../i18n";
import { useStore } from "../store";
import { Button, Field, inputClass, Modal } from "./Modal";

export function ShareDialog({ tab, row }: { tab: string; row: number | null }) {
  const editor = useEditors((s) => s.tabs[tab]);
  const result = editor?.outcome?.result ?? null;
  const fetched = result?.rows ?? 0;
  const [which, setWhich] = useState<"current" | "fetched">(row !== null ? "current" : "fetched");
  const [label, setLabel] = useState(() => editorLabel(tab));
  const [error, setError] = useState<string | null>(null);
  const [sharing, setSharing] = useState(false);
  const close = () => useStore.getState().openDialog(null);
  if (!editor?.outcome || !result) return null;
  const { executed } = editor.outcome;
  const count = which === "current" ? 1 : Math.min(fetched, SHARE_ROWS);

  const submit = async () => {
    setError(null);
    setSharing(true);
    try {
      const rows =
        which === "current" && row !== null ? await api.resultRows(result.id, row, 1) : await api.resultRows(result.id, 0, count);
      const columns = executed.columns.map((c) => ({ name: c.name, type: c.typeName }));
      const shared = await share(editor.registration, label.trim() || editorLabel(tab), columns, rows);
      close();
      if (shared < rows.length) {
        useStore.getState().setNotice(t("Only the first {n} rows were shared: the rest went over the size a share can have.", { n: shared }));
      }
    } catch (e) {
      setError(errorText(e));
      setSharing(false);
    }
  };

  return (
    <Modal
      title={t("Share with Claude Code")}
      width="w-[30rem]"
      onCancel={close}
      onSubmit={() => void submit()}
      footer={
        <>
          <span className="flex-1" />
          <Button onClick={close}>{t("Cancel")}</Button>
          <Button primary submit disabled={sharing}>
            {count === 1 ? t("Share 1 Row") : t("Share {n} Rows", { n: count.toLocaleString() })}
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-2.5">
        <label className="flex items-center gap-2">
          <input
            type="radio"
            name="share-which"
            checked={which === "current"}
            disabled={row === null}
            onChange={() => setWhich("current")}
            className="accent-[var(--accent)]"
          />
          {t("The current record")}
        </label>
        <label className="flex items-center gap-2">
          <input
            type="radio"
            name="share-which"
            checked={which === "fetched"}
            onChange={() => setWhich("fetched")}
            className="accent-[var(--accent)]"
          />
          {fetched > SHARE_ROWS
            ? t("The first {n} rows fetched", { n: SHARE_ROWS.toLocaleString() })
            : t("The {n} rows fetched", { n: fetched.toLocaleString() })}
        </label>
        <Field label={t("Called")} hint={t("What the agent is told the rows are")}>
          <input className={inputClass} value={label} onChange={(e) => setLabel(e.target.value)} />
        </Field>
        <p className="text-muted">
          {t(
            "All {columns} columns of these rows go to Claude Code, and from it to Anthropic under your account: their values leave this computer. They are kept for this run of Claude Code only.",
            { columns: executed.columns.length },
          )}
        </p>
        {error && <p className="rounded border border-danger/40 px-2.5 py-1.5 text-danger">{error}</p>}
      </div>
    </Modal>
  );
}
