// The selected blob cell's content. The grid shows (MEMO) or (BLOB); this
// reads the one cell the cursor is on, in the transaction that fetched it.
import { useEffect, useState } from "react";
import { api, errorText, type BlobView } from "../api";
import { t } from "../i18n";

/** Read at most this much: a viewer, not an export */
const MAX = 1 << 20;

type State = { status: "loading" } | { status: "null" } | { status: "error"; error: string } | { status: "ok"; blob: BlobView };

export function BlobViewer({ result, row, col }: { result: number; row: number; col: number }) {
  const [state, setState] = useState<State>({ status: "loading" });

  useEffect(() => {
    let live = true;
    setState({ status: "loading" });
    // Moving through rows quickly reads only where the cursor stops
    const timer = setTimeout(() => {
      api
        .resultBlob(result, row, col, MAX)
        .then((blob) => live && setState(blob ? { status: "ok", blob } : { status: "null" }))
        .catch((e) => live && setState({ status: "error", error: errorText(e) }));
    }, 150);
    return () => {
      live = false;
      clearTimeout(timer);
    };
  }, [result, row, col]);

  const content = state.status === "ok" ? (state.blob.text ?? state.blob.hex ?? "") : "";
  let header: string;
  switch (state.status) {
    case "loading":
      header = t("Reading…");
      break;
    case "null":
      header = t("<null>");
      break;
    case "error":
      header = t("Cannot read the blob");
      break;
    case "ok": {
      const kind = state.blob.text !== null ? t("Text") : t("Binary");
      const size = t("{n} bytes", { n: state.blob.size.toLocaleString() });
      header = state.blob.complete ? `${kind} · ${size}` : `${kind} · ${t("first {size}", { size })}`;
    }
  }

  return (
    <div className="flex h-full flex-col">
      <div className="flex h-7 shrink-0 items-center gap-2 border-b border-line bg-panel-2 px-2 text-muted">
        <span className="font-semibold text-fg">{t("Blob")}</span>
        <span className="truncate">{header}</span>
        <span className="flex-1" />
        {state.status === "ok" && (
          <button
            className="rounded px-1.5 hover:bg-hover hover:text-fg"
            onClick={() => void navigator.clipboard.writeText(content)}
          >
            {t("Copy")}
          </button>
        )}
      </div>
      {state.status === "error" ? (
        <p className="p-2 text-danger">{state.error}</p>
      ) : (
        <pre className="scroll-thin min-h-0 flex-1 overflow-auto p-2 font-mono text-[12px] whitespace-pre-wrap select-text">
          {content}
        </pre>
      )}
    </div>
  );
}
