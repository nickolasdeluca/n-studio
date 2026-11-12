// A change to the catalog, before it is made: the script that makes it, for
// the user to read, and then run whole or not at all.
import { useState } from "react";
import { errorText, type ScriptProblem } from "../api";
import { metadataChanged } from "../changes";
import { closePanel, openEditor } from "../dock";
import { t } from "../i18n";
import { defaultTx, newRun, runScript } from "../script";
import { useStore, type Dialog } from "../store";
import { useViewers } from "../viewers";
import { Button, Modal } from "./Modal";

type Compile = Extract<Dialog, { type: "compile" }>;

interface Failure {
  /** What the server refused, with the statement it refused */
  problems: ScriptProblem[];
  /** What went wrong outside any statement */
  note?: string;
}

export function CompileDialog({ dialog }: { dialog: Compile }) {
  const { registration, script } = dialog;
  const [running, setRunning] = useState(false);
  const [failure, setFailure] = useState<Failure | null>(null);
  const close = () => useStore.getState().openDialog(null);
  // Back to the form it came from, with what was typed in it
  const cancel = () => useStore.getState().openDialog(dialog.back ?? null);

  const compile = async () => {
    if (running) return;
    setRunning(true);
    setFailure(null);
    const problems: ScriptProblem[] = [];
    try {
      const options = { stopOnError: true, autoDdl: false, atomic: true };
      const end = await runScript(newRun(), registration, null, defaultTx, script, options, (e) => problems.push(...e.problems));
      const errors = [...problems, ...end.problems].filter((p) => p.error !== null);
      if (errors.length === 0 && end.ended === "commit") {
        metadataChanged(registration);
        dialog.done?.();
        close();
        return;
      }
      setFailure({
        problems: errors,
        note: errors.length > 0 ? t("Nothing was changed: the transaction was rolled back.") : t("There is nothing to run in this script."),
      });
    } catch (e) {
      setFailure({ problems: [], note: errorText(e) });
    } finally {
      setRunning(false);
    }
  };

  const toEditor = () => {
    openEditor(registration, script);
    close();
  };

  return (
    <Modal
      title={dialog.title}
      width="w-[46rem]"
      onCancel={cancel}
      onSubmit={() => void compile()}
      footer={
        <>
          <Button onClick={toEditor}>{t("Open in SQL Editor")}</Button>
          <span className="flex-1" />
          {running && <span className="text-muted" role="status">{t("Running…")}</span>}
          <Button onClick={cancel} autoFocus={dialog.danger}>
            {dialog.back ? t("Back") : t("Cancel")}
          </Button>
          <Button primary={!dialog.danger} danger={dialog.danger} submit disabled={running} autoFocus={!dialog.danger}>
            {dialog.verb ?? (dialog.danger ? t("Run and Commit") : failure ? t("Compile Again") : t("Compile and Commit"))}
          </Button>
        </>
      }
    >
      <p className="mb-2 text-muted">
        {t("These statements run in one transaction: committed if all of them succeed, rolled back otherwise.")}
      </p>
      <pre className="scroll-thin max-h-[22rem] overflow-auto rounded border border-line bg-bg p-2 font-mono text-[12px] select-text">
        {script}
      </pre>
      {failure && (
        <div className="mt-3 flex flex-col gap-2" role="alert">
          {failure.problems.map((p) => (
            <div key={p.index} className="rounded border border-danger/40 bg-danger/5 p-2">
              <div className="whitespace-pre-wrap text-danger select-text">{p.error}</div>
              <div className="mt-1 truncate font-mono text-[12px] text-muted" title={script.slice(p.from, p.to)}>
                {t("Statement {n}: {sql}", { n: p.index + 1, sql: script.slice(p.from, p.to).replace(/\s+/g, " ") })}
              </div>
            </div>
          ))}
          {failure.note && <p className={failure.problems.length > 0 ? "text-muted" : "text-danger"}>{failure.note}</p>}
        </div>
      )}
    </Modal>
  );
}

/** A viewer closing over text typed and not compiled */
export function DiscardDialog({ viewer }: { viewer: string }) {
  const name = useViewers((s) => s.viewers[viewer]?.name);
  const close = () => useStore.getState().openDialog(null);
  const discard = () => {
    close();
    closePanel(viewer);
  };
  return (
    <Modal
      title={t("Close {name}", { name: name ?? "" })}
      width="w-[26rem]"
      onCancel={close}
      onSubmit={discard}
      footer={
        <>
          <span className="flex-1" />
          <Button onClick={close} autoFocus>
            {t("Keep Editing")}
          </Button>
          <Button danger submit>
            {t("Discard Changes")}
          </Button>
        </>
      }
    >
      <p>{t("{name} has changes that were not compiled. Closing it discards them.", { name: name ?? "" })}</p>
    </Modal>
  );
}
