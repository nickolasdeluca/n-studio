// What a select brought back: the record navigator over the grid, with the
// blob viewer beside it. An editor's Results tab and an object viewer's Data
// tab are both this.
import { useCallback, useRef, type ReactNode } from "react";
import type { ColumnKind } from "../api";
import { useEditors, type EditorTab } from "../editors";
import { t } from "../i18n";
import { Icon } from "../icons";
import { useStore } from "../store";
import { BlobViewer } from "./BlobViewer";
import { editCount } from "../rowEdits";
import { ResultGrid, type GridCursor, type GridEdit, type GridHandle } from "./ResultGrid";

/** A grid whose rows can be changed, and what is done with the changes */
export interface ResultsEdit extends GridEdit {
  /** Shows the changes as the script that makes them, to be run */
  onApply(): void;
  onRevert(): void;
}

export function Empty({ children }: { children: ReactNode }) {
  return <div className="grid h-full place-items-center p-4 text-center text-muted">{children}</div>;
}

export function Results(props: { tab: EditorTab; cursor: GridCursor; setCursor(c: GridCursor): void; edit?: ResultsEdit }) {
  const { tab, cursor, setCursor, edit } = props;
  const grid = useRef<GridHandle>(null);
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
    setCursor({ row: Math.max(0, added + rows - 1), col: cursor.col });
  };
  // New rows come before the fetched ones
  const added = edit?.edits.inserted.length ?? 0;
  const total = result.rows + added;
  const go = (row: number) => setCursor({ row: Math.max(0, Math.min(row, total - 1)), col: cursor.col });
  const pending = edit ? editCount(edit.edits) : 0;
  const onNew = cursor.row < added;

  return (
    <div className="flex h-full flex-col">
      <div className="flex h-7 shrink-0 items-center gap-0.5 border-b border-line px-1.5 text-muted">
        <Nav icon="first" label={t("First Record")} disabled={cursor.row === 0} onClick={() => go(0)} />
        <Nav icon="prev" label={t("Prior Record")} disabled={cursor.row === 0} onClick={() => go(cursor.row - 1)} />
        <Nav icon="next" label={t("Next Record")} disabled={cursor.row >= total - 1} onClick={() => go(cursor.row + 1)} />
        <Nav
          icon="last"
          label={t("Last Record (fetches all)")}
          disabled={tab.running || tab.fetching || (result.eof && cursor.row >= total - 1)}
          onClick={() => (result.eof ? go(total - 1) : fetchAll())}
        />
        <Nav icon="fetch-all" label={t("Fetch All")} disabled={tab.running || tab.fetching || result.eof} onClick={fetchAll} />
        <span className="ml-2 tabular-nums">
          {total === 0
            ? t("No records")
            : onNew
              ? t("New record")
              : t("Record {n} of {total}", {
                  n: (cursor.row - added + 1).toLocaleString(),
                  total: result.eof ? result.rows.toLocaleString() : `${result.rows.toLocaleString()}+`,
                })}
        </span>
        {edit && (
          <>
            <span className="mx-1.5 h-4 w-px bg-line" />
            <Nav icon="plus" label={t("Insert Record (Insert)")} onClick={() => grid.current?.insertRow()} />
            <Nav
              icon="trash"
              label={t("Delete Record, or Keep It (Ctrl+Delete)")}
              disabled={total === 0}
              onClick={() => grid.current?.deleteRow()}
            />
            <button
              type="button"
              title={t("Set to NULL (Shift+Delete)")}
              disabled={total === 0 || !edit.editable[cursor.col]}
              onClick={() => grid.current?.setNull()}
              className="h-6 rounded px-1.5 font-mono text-[11px] text-fg hover:bg-hover disabled:text-faint disabled:hover:bg-transparent"
            >
              NULL
            </button>
          </>
        )}
        {result.endedEarly && <span className="ml-2 text-faint">{t("(the transaction ended before all rows were fetched)")}</span>}
        <span className="flex-1" />
        {edit && pending > 0 && (
          <>
            <span className="mr-1 text-fg tabular-nums" role="status">
              {pending === 1 ? t("1 row changed, not applied") : t("{n} rows changed, not applied", { n: pending })}
            </span>
            <Nav icon="rollback" label={t("Discard the Changes")} onClick={edit.onRevert} />
            <button
              type="button"
              onClick={edit.onApply}
              className="mr-1 flex h-6 items-center gap-1.5 rounded px-2 text-fg hover:bg-hover"
            >
              <Icon name="commit" size={13} className="text-ok" />
              {t("Apply…")}
            </button>
          </>
        )}
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
            edit={edit}
            ref={grid}
          />
        </div>
        {isBlob(executed.columns[cursor.col]?.kind) && !onNew && cursor.row < total && (
          <div className="w-[38%] min-w-64 shrink-0 border-l border-line">
            <BlobViewer result={result.id} row={cursor.row - added} col={cursor.col} />
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
