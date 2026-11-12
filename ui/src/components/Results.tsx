// What a select brought back: the record navigator over the grid, with the
// blob viewer beside it. An editor's Results tab and an object viewer's Data
// tab are both this.
import { useCallback, type ReactNode } from "react";
import type { ColumnKind } from "../api";
import { useEditors, type EditorTab } from "../editors";
import { t } from "../i18n";
import { Icon } from "../icons";
import { useStore } from "../store";
import { BlobViewer } from "./BlobViewer";
import { ResultGrid, type GridCursor } from "./ResultGrid";

export function Empty({ children }: { children: ReactNode }) {
  return <div className="grid h-full place-items-center p-4 text-center text-muted">{children}</div>;
}

export function Results({ tab, cursor, setCursor }: { tab: EditorTab; cursor: GridCursor; setCursor(c: GridCursor): void }) {
  const outcome = tab.outcome;
  const fetchMore = useCallback((max: number) => useEditors.getState().fetchMore(tab.id, max), [tab.id]);
  const sort = useCallback((col: number) => void useEditors.getState().sort(tab.id, col), [tab.id]);
  if (!outcome) return <Empty>{t("Execute a statement with F9 or Ctrl+Enter.")}</Empty>;
  const { executed, result } = outcome;

  if (!result) {
    if (executed.rows.length > 0) return <StaticRows columns={executed.columns.map((c) => c.name)} rows={executed.rows} />;
    return <Empty>{t("The statement returned no rows.")}</Empty>;
  }

  const fetchAll = async () => {
    await fetchMore(Number.MAX_SAFE_INTEGER);
    const rows = useEditors.getState().tabs[tab.id]?.outcome?.result?.rows ?? 0;
    setCursor({ row: Math.max(0, rows - 1), col: cursor.col });
  };
  const go = (row: number) => setCursor({ row: Math.max(0, Math.min(row, result.rows - 1)), col: cursor.col });

  return (
    <div className="flex h-full flex-col">
      <div className="flex h-7 shrink-0 items-center gap-0.5 border-b border-line px-1.5 text-muted">
        <Nav icon="first" label={t("First Record")} disabled={cursor.row === 0} onClick={() => go(0)} />
        <Nav icon="prev" label={t("Prior Record")} disabled={cursor.row === 0} onClick={() => go(cursor.row - 1)} />
        <Nav icon="next" label={t("Next Record")} disabled={cursor.row >= result.rows - 1} onClick={() => go(cursor.row + 1)} />
        <Nav
          icon="last"
          label={t("Last Record (fetches all)")}
          disabled={tab.running || tab.fetching || (result.eof && cursor.row >= result.rows - 1)}
          onClick={() => (result.eof ? go(result.rows - 1) : fetchAll())}
        />
        <Nav icon="fetch-all" label={t("Fetch All")} disabled={tab.running || tab.fetching || result.eof} onClick={fetchAll} />
        <span className="ml-2 tabular-nums">
          {result.rows === 0
            ? t("No records")
            : t("Record {n} of {total}", {
                n: (cursor.row + 1).toLocaleString(),
                total: result.eof ? result.rows.toLocaleString() : `${result.rows.toLocaleString()}+`,
              })}
        </span>
        {result.endedEarly && <span className="ml-2 text-faint">{t("(the transaction ended before all rows were fetched)")}</span>}
        <span className="flex-1" />
        <Nav
          icon="export"
          label={t("Export Rows… (fetches all)")}
          disabled={tab.running || tab.fetching || result.rows === 0}
          onClick={() => useStore.getState().openDialog({ type: "export", tab: tab.id })}
        />
      </div>
      <div className="flex min-h-0 flex-1">
        <div className="min-w-0 flex-1">
          <ResultGrid
            result={result}
            columns={executed.columns}
            cursor={cursor}
            onCursor={setCursor}
            onFetchMore={fetchMore}
            onSort={sort}
          />
        </div>
        {isBlob(executed.columns[cursor.col]?.kind) && result.rows > 0 && (
          <div className="w-[38%] min-w-64 shrink-0 border-l border-line">
            <BlobViewer result={result.id} row={cursor.row} col={cursor.col} />
          </div>
        )}
      </div>
    </div>
  );
}

const isBlob = (kind: ColumnKind | undefined) => kind === "blob" || kind === "text_blob";

function Nav(props: { icon: string; label: string; disabled?: boolean; onClick(): unknown }) {
  return (
    <button
      title={props.label}
      aria-label={props.label}
      disabled={props.disabled}
      onClick={() => void props.onClick()}
      className="grid size-6 place-items-center rounded text-fg hover:bg-hover disabled:text-faint disabled:hover:bg-transparent"
    >
      <Icon name={props.icon} size={13} />
    </button>
  );
}

/** A handful of rows held here: an EXECUTE PROCEDURE's outputs */
function StaticRows({ columns, rows }: { columns: string[]; rows: (string | null)[][] }) {
  return (
    <div className="scroll-thin h-full overflow-auto">
      <table className="border-collapse font-mono text-[12px]">
        <thead>
          <tr>
            {columns.map((c, i) => (
              <th key={i} className="sticky top-0 border-r border-b border-line bg-panel-2 px-2 text-left font-sans font-semibold text-muted">
                {c}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {rows.map((r, i) => (
            <tr key={i}>
              {r.map((v, j) => (
                <td key={j} className="border-r border-b border-line/60 px-2">
                  {v === null ? <span className="text-faint italic">{t("<null>")}</span> : v}
                </td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
