// The selected blob cell's content. The grid shows (MEMO) or (BLOB); this
// reads the one cell the cursor is on, in the transaction that fetched it,
// and shows it the ways it can be read: as text, as a picture, as indented
// JSON or XML, or as the bytes it is.
import { useEffect, useMemo, useState } from "react";
import { save } from "@tauri-apps/plugin-dialog";
import { api, errorText, type BlobView } from "../api";
import { extensionOf, fromBase64, hexDump, isSvg, jsonOf, pictureOf, textBase64, textOf, xmlOf } from "../blob";
import { t } from "../i18n";

/** Read at most this much: a viewer. Save writes the whole blob. */
const MAX = 8 << 20;
/** Draw at most this much text, and this many bytes of a dump */
const TEXT_SHOWN = 2 << 20;
const HEX_SHOWN = 64 << 10;

type View = "text" | "picture" | "json" | "xml" | "hex";
const VIEWS: [View, string][] = [
  ["text", "Text"],
  ["picture", "Picture"],
  ["json", "JSON"],
  ["xml", "XML"],
  ["hex", "Hex"],
];

type State = { status: "loading" } | { status: "null" } | { status: "error"; error: string } | { status: "ok"; blob: BlobView };

/** A read blob, in every way it can be shown */
interface Shown {
  /** What the server calls it */
  kind: string;
  views: Partial<Record<View, string>>;
  /** The view it opens in */
  first: View;
  /** For the file it is saved as */
  extension: string;
  /** Said beside the size: the encoding guessed, what was left undrawn */
  notes: string[];
}

function shown(blob: BlobView): Shown {
  const views: Partial<Record<View, string>> = {};
  const notes: string[] = [];
  let extension = "bin";
  let text = blob.text;
  if (blob.data !== null) {
    const bytes = fromBase64(blob.data);
    const picture = pictureOf(bytes);
    if (picture) {
      views.picture = `data:${picture.mime};base64,${blob.data}`;
      extension = picture.extension;
    } else {
      // Memos kept in a subtype 0 blob, as older schemas do
      const decoded = textOf(bytes);
      if (decoded) {
        text = decoded.text;
        notes.push(decoded.encoding);
      }
      extension = extensionOf(bytes) ?? (decoded ? "txt" : "bin");
    }
    views.hex = hexDump(bytes.subarray(0, HEX_SHOWN));
    if (bytes.length > HEX_SHOWN) views.hex += `\n… ${t("{n} more bytes", { n: (bytes.length - HEX_SHOWN).toLocaleString() })}`;
  } else {
    extension = "txt";
  }
  if (text !== null) {
    views.text = text.length > TEXT_SHOWN ? text.slice(0, TEXT_SHOWN) : text;
    if (text.length > TEXT_SHOWN) notes.push(t("first {n} characters shown", { n: TEXT_SHOWN.toLocaleString() }));
    if (isSvg(text) && blob.complete) {
      views.picture = `data:image/svg+xml;base64,${textBase64(text)}`;
      extension = "svg";
    }
    const json = text.length <= TEXT_SHOWN ? jsonOf(text) : null;
    const xml = json === null && text.length <= TEXT_SHOWN ? xmlOf(text) : null;
    if (json !== null) {
      views.json = json;
      extension = "json";
    } else if (xml !== null) {
      views.xml = xml;
      if (extension === "txt") extension = "xml";
    }
  }
  const first: View = views.picture && blob.data !== null ? "picture" : views.text !== undefined ? "text" : "hex";
  return { kind: blob.text !== null ? t("Text") : t("Binary"), views, first, extension, notes };
}

export function BlobViewer({ result, row, col }: { result: number; row: number; col: number }) {
  const [state, setState] = useState<State>({ status: "loading" });
  // The view asked for stays as the cursor moves, wherever a blob has it
  const [asked, setAsked] = useState<View | null>(null);
  const [pixels, setPixels] = useState("");
  const [note, setNote] = useState("");

  useEffect(() => {
    let live = true;
    setState({ status: "loading" });
    setPixels("");
    setNote("");
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

  const blob = state.status === "ok" ? state.blob : null;
  const content = useMemo(() => (blob ? shown(blob) : null), [blob]);
  const view: View | null = content ? (asked && content.views[asked] !== undefined ? asked : content.first) : null;
  const body = content && view ? (content.views[view] ?? "") : "";

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
      const size = t("{n} bytes", { n: state.blob.size.toLocaleString() });
      const parts = [content!.kind, state.blob.complete ? size : t("first {size}", { size }), ...content!.notes];
      if (view === "picture" && pixels) parts.push(pixels);
      header = parts.join(" · ");
    }
  }

  const saveAs = async () => {
    if (!content) return;
    try {
      const path = await save({ title: t("Save Blob"), defaultPath: `blob.${content.extension}` });
      if (!path) return;
      const written = await api.resultBlobSave(result, row, col, path);
      setNote(t("Saved: {n} bytes", { n: written.toLocaleString() }));
    } catch (e) {
      setNote(errorText(e));
    }
  };

  const button = "rounded px-1.5 hover:bg-hover hover:text-fg";
  return (
    <div className="flex h-full flex-col">
      <div className="flex h-7 shrink-0 items-center gap-2 border-b border-line bg-panel-2 px-2 text-muted">
        <span className="font-semibold text-fg">{t("Blob")}</span>
        <span className="truncate" title={note || header}>
          {note || header}
        </span>
        <span className="flex-1" />
        {content && (
          <>
            {view !== "picture" && (
              <button className={button} onClick={() => void navigator.clipboard.writeText(body)}>
                {t("Copy")}
              </button>
            )}
            <button className={button} title={t("Write the whole blob to a file")} onClick={() => void saveAs()}>
              {t("Save…")}
            </button>
          </>
        )}
      </div>
      {content && Object.keys(content.views).length > 1 && (
        <div className="flex h-6 shrink-0 items-center gap-1 border-b border-line px-1 text-muted">
          {VIEWS.filter(([id]) => content.views[id] !== undefined).map(([id, label]) => (
            <button
              key={id}
              className={`rounded px-1.5 ${id === view ? "bg-hover text-fg" : "hover:bg-hover hover:text-fg"}`}
              onClick={() => setAsked(id)}
            >
              {t(label)}
            </button>
          ))}
        </div>
      )}
      {state.status === "error" ? (
        <p className="p-2 text-danger">{state.error}</p>
      ) : view === "picture" ? (
        <div className="scroll-thin flex min-h-0 flex-1 items-start justify-center overflow-auto p-2">
          <img
            src={body}
            alt=""
            className="max-w-full"
            onLoad={(e) => setPixels(`${e.currentTarget.naturalWidth} × ${e.currentTarget.naturalHeight}`)}
            onError={() => setNote(t("The picture cannot be drawn: its bytes are in Hex"))}
          />
        </div>
      ) : (
        <pre
          className={`scroll-thin min-h-0 flex-1 overflow-auto p-2 font-mono text-[12px] select-text ${view === "hex" ? "whitespace-pre" : "whitespace-pre-wrap"}`}
        >
          {body}
        </pre>
      )}
    </div>
  );
}
