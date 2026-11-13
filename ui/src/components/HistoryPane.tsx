// An editor's History tab: what was run on its database, the latest first.
// A double click puts a statement back into the editor.
import { useEffect, useState } from "react";
import { api, errorText, type HistoryEntry } from "../api";
import { openEditor } from "../dock";
import { ms } from "../editors";
import { historyChanged, onHistory } from "../history";
import { t } from "../i18n";
import { useStore } from "../store";
import { Empty } from "./Results";

/** Entries shown at once; searching reaches the older ones */
const SHOWN = 300;

const flat = (sql: string) => sql.replace(/\s+/g, " ").slice(0, 400);

function when(at: number): string {
  const date = new Date(at);
  const today = new Date().toDateString() === date.toDateString();
  return today ? date.toLocaleTimeString() : `${date.toLocaleDateString()} ${date.toLocaleTimeString()}`;
}

export function HistoryPane({ registration, insert }: { registration: string; insert(text: string): void }) {
  const [search, setSearch] = useState("");
  const [entries, setEntries] = useState<HistoryEntry[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [version, setVersion] = useState(0);
  const [confirming, setConfirming] = useState(false);
  const connected = useStore((s) => !!s.servers[registration]);

  useEffect(() => onHistory((changed) => changed === registration && setVersion((v) => v + 1)), [registration]);

  useEffect(() => {
    let current = true;
    // Typing searches once it pauses
    const timer = setTimeout(
      () => {
        api.historyList(registration, search, SHOWN).then(
          (list) => current && (setEntries(list), setError(null)),
          (e) => current && setError(errorText(e)),
        );
      },
      search ? 200 : 0,
    );
    return () => {
      current = false;
      clearTimeout(timer);
    };
  }, [registration, search, version]);

  const clear = async () => {
    setConfirming(false);
    try {
      await api.historyClear(registration);
      historyChanged(registration);
    } catch (e) {
      setError(errorText(e));
    }
  };

  let body;
  if (error) body = <Empty>{error}</Empty>;
  else if (!entries) body = null;
  else if (entries.length === 0) body = <Empty>{search ? t("Nothing in the history has that.") : t("Nothing was run on this database yet.")}</Empty>;
  else {
    body = (
      <div className="scroll-thin h-full overflow-auto font-mono text-[12px]">
        {entries.map((e) => (
          <div
            key={e.id}
            onDoubleClick={() => insert(e.sql)}
            title={e.error ? `${e.error}\n\n${e.sql.slice(0, 2000)}` : e.sql.slice(0, 2000)}
            className="group flex cursor-default items-baseline gap-3 px-2 py-0.5 hover:bg-hover"
          >
            <span className="w-36 shrink-0 text-faint tabular-nums">{when(e.at)}</span>
            <span className={`w-20 shrink-0 text-right tabular-nums ${e.error ? "text-danger" : "text-muted"}`}>
              {e.error ? t("failed") : ms(e.elapsedMs)}
            </span>
            <span className="w-8 shrink-0 text-faint tabular-nums">{e.runs > 1 ? `×${e.runs}` : ""}</span>
            <span className="min-w-0 flex-1 truncate">{flat(e.sql)}</span>
            <button
              className="hidden shrink-0 rounded px-1.5 font-sans text-[11.5px] text-muted hover:bg-panel hover:text-fg group-hover:block disabled:hidden"
              disabled={!connected}
              onClick={() => openEditor(registration, e.sql)}
            >
              {t("New Editor")}
            </button>
          </div>
        ))}
        {entries.length === SHOWN && (
          <div className="px-2 py-1.5 font-sans text-faint">{t("The latest {n} are shown; search to reach older ones.", { n: SHOWN })}</div>
        )}
      </div>
    );
  }

  return (
    <div className="flex h-full flex-col">
      <div className="flex h-7 shrink-0 items-center gap-2 border-b border-line px-2">
        <input
          className="h-5 w-64 rounded border border-line bg-panel px-1.5 text-fg placeholder:text-faint"
          placeholder={t("Search the history")}
          value={search}
          onChange={(e) => setSearch(e.target.value)}
        />
        <span className="text-faint">{t("Double-click puts a statement into the editor.")}</span>
        <span className="flex-1" />
        {confirming ? (
          <>
            <span className="text-muted">{t("Forget everything run on this database?")}</span>
            <button className="rounded px-1.5 text-danger hover:bg-hover" onClick={() => void clear()}>
              {t("Clear")}
            </button>
            <button className="rounded px-1.5 text-muted hover:bg-hover" onClick={() => setConfirming(false)}>
              {t("Cancel")}
            </button>
          </>
        ) : (
          <button
            className="rounded px-1.5 text-muted hover:bg-hover disabled:text-faint disabled:hover:bg-transparent"
            disabled={!entries?.length && !search}
            onClick={() => setConfirming(true)}
          >
            {t("Clear History…")}
          </button>
        )}
      </div>
      <div className="min-h-0 flex-1">{body}</div>
    </div>
  );
}
