// What a select brought back: the record navigator over the grid, with the
// blob viewer beside it. An editor's Results tab and an object viewer's Data
// tab are both this.
import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { create } from "zustand";
import type { ColumnInfo, ColumnKind, ColumnSummary } from "../api";
import { AGGREGATES, aggregatesFor, shortOf, valueOf, type Aggregate } from "../summary";
import { useContextMenu } from "./ContextMenu";
import { useAgent } from "../agent";
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

const SUMMARY_ROW = "n-studio.summaryRow";

/** Whether grids show their summary row, the same for all of them and kept
 *  across runs; and what each editor's columns add up to, by column name,
 *  so that a select run again keeps them */
const useSummaryRow = create<{
  shown: boolean;
  picks: Record<string, Record<string, Aggregate>>;
  toggle(): void;
  pick(tab: string, column: string, aggregate: Aggregate | null): void;
}>()((set) => ({
  shown: (() => {
    try {
      return localStorage.getItem(SUMMARY_ROW) === "1";
    } catch {
      return false;
    }
  })(),
  picks: {},
  toggle: () =>
    set((s) => {
      try {
        localStorage.setItem(SUMMARY_ROW, s.shown ? "0" : "1");
      } catch {
        // Shown for this run only
      }
      return { shown: !s.shown };
    }),
  pick: (tab, column, aggregate) =>
    set((s) => {
      const picks = { ...s.picks[tab] };
      if (aggregate) picks[column] = aggregate;
      else delete picks[column];
      return { picks: { ...s.picks, [tab]: picks } };
    }),
}));

const NO_PICKS: Record<string, Aggregate> = {};

export function Results(props: { tab: EditorTab; cursor: GridCursor; setCursor(c: GridCursor): void; edit?: ResultsEdit }) {
  const { tab, cursor, setCursor, edit } = props;
  const grid = useRef<GridHandle>(null);
  const outcome = tab.outcome;
  const fetchMore = useCallback((max: number) => useEditors.getState().fetchMore(tab.id, max), [tab.id]);
  const sort = useCallback((col: number) => void useEditors.getState().sort(tab.id, col), [tab.id]);
  const summaryShown = useSummaryRow((s) => s.shown);
  const picks = useSummaryRow((s) => s.picks[tab.id] ?? NO_PICKS);
  const summaries = useSummaries(tab, summaryShown, picks);
  const agentRunning = useAgent((s) => s.status === "running");
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
        <button
          type="button"
          title={t("Summary Row: what each column adds up to")}
          aria-label={t("Summary Row")}
          aria-pressed={summaryShown}
          onClick={() => useSummaryRow.getState().toggle()}
          className={`grid size-6 place-items-center rounded hover:bg-hover ${summaryShown ? "bg-selected-inactive text-accent" : "text-fg"}`}
        >
          <Icon name="sum" size={13} />
        </button>
        {agentRunning && (
          <Nav
            icon="agent"
            label={t("Share with Claude Code…")}
            disabled={tab.running || tab.fetching || result.rows === 0}
            onClick={() =>
              useStore.getState().openDialog({ type: "share", tab: tab.id, row: onNew || total === 0 ? null : cursor.row - added })
            }
          />
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
            footer={
              summaryShown
                ? {
                    gutter: <Icon name="sum" size={12} />,
                    cell: (col) => (
                      <SummaryCell
                        column={executed.columns[col]}
                        aggregate={picks[executed.columns[col].name] ?? null}
                        summary={summaries.by[col]}
                        eof={result.eof}
                        edited={pending > 0}
                        onOpen={(at) => summaries.menu(col, at)}
                      />
                    ),
                  }
                : undefined
            }
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

/** What the picked columns of a result add up to, worked out once every
 *  row is fetched; and the menu a summary cell opens */
function useSummaries(tab: EditorTab, shown: boolean, picks: Record<string, Aggregate>) {
  const result = tab.outcome?.result ?? null;
  const columns = tab.outcome?.executed.columns;
  const [held, setHeld] = useState<{ result: number; by: Record<number, ColumnSummary> }>({ result: -1, by: {} });
  const by = held.result === result?.id ? held.by : {};
  const showMenu = useContextMenu((m) => m.show);

  const work = useCallback(
    async (cols: number[]) => {
      if (!result || cols.length === 0) return {};
      const got = await useEditors.getState().summarize(tab.id, cols);
      if (!got) return null;
      const more = Object.fromEntries(cols.map((c, i) => [c, got[i]]));
      setHeld((h) => ({ result: result.id, by: { ...(h.result === result.id ? h.by : {}), ...more } }));
      return more;
    },
    [tab.id, result],
  );

  // Once every row is here, the picked columns are worked out on their own:
  // before, that would fetch every row of a select nobody asked to
  const missing = shown && result?.eof && columns ? columns.flatMap((c, i) => (picks[c.name] && !by[i] ? [i] : [])) : [];
  const key = missing.length > 0 ? `${result?.id}:${missing.join(",")}` : "";
  const busy = tab.running || tab.fetching;
  /** Asked for and not answered yet: the fetch ends before the answer comes */
  const asked = useRef("");
  useEffect(() => {
    if (!key || busy || asked.current === key) return;
    asked.current = key;
    void work(missing).finally(() => {
      if (asked.current === key) asked.current = "";
    });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key, busy, work]);

  const menu = async (col: number, at: DOMRect) => {
    const column = columns?.[col];
    if (!column || busy) return;
    const summary = by[col] ?? (await work([col]))?.[col];
    const current = useSummaryRow.getState().picks[tab.id]?.[column.name] ?? null;
    const pick = (a: Aggregate | null) => useSummaryRow.getState().pick(tab.id, column.name, a);
    const value = summary && current ? valueOf(summary, current) : null;
    showMenu(at.left, at.top, [
      { label: t("None"), icon: current ? undefined : "commit", onSelect: () => pick(null) },
      ...aggregatesFor(column.kind).map((a) => {
        const v = summary ? valueOf(summary, a) : null;
        const label = AGGREGATES.find((x) => x.value === a)!.label;
        return {
          label: summary ? t("{label} = {value}", { label: t(label), value: v ?? "—" }) : t(label),
          icon: current === a ? "commit" : undefined,
          onSelect: () => pick(a),
        };
      }),
      ...(value !== null
        ? ["separator" as const, { label: t("Copy {value}", { value }), icon: "copy", onSelect: () => void navigator.clipboard.writeText(value) }]
        : []),
    ], true);
  };

  return { by, menu };
}

/** A column's cell in the summary row: what it adds up to, or a place to
 *  choose that */
function SummaryCell(props: {
  column: ColumnInfo;
  aggregate: Aggregate | null;
  summary: ColumnSummary | undefined;
  eof: boolean;
  /** Rows were changed and not applied: the summary is of them as read */
  edited: boolean;
  onOpen(at: DOMRect): void;
}) {
  const { column, aggregate, summary } = props;
  const value = aggregate && summary ? valueOf(summary, aggregate) : null;
  const shown = !aggregate ? "" : summary ? (value ?? (summary.overflow ? t("too large") : "—")) : "?";
  const hint = !aggregate
    ? t("Choose what {column} adds up to", { column: column.name })
    : !summary && !props.eof
      ? t("Click to fetch every row and add them up")
      : summary?.overflow && aggregate === "sum"
        ? t("The sum is past 38 digits")
        : props.edited
          ? t("Of the rows as they were read, without the changes not applied")
          : t("Of every row");
  return (
    <button
      type="button"
      title={aggregate ? `${shortOf(aggregate)} = ${shown}\n${hint}` : hint}
      onClick={(e) => props.onOpen(e.currentTarget.getBoundingClientRect())}
      className={`group flex h-full w-full items-center gap-1 truncate px-1.5 font-mono text-[12px] hover:bg-hover ${
        aggregate ? "text-fg" : "text-faint"
      } ${column.kind === "integer" || column.kind === "decimal" || column.kind === "float" ? "justify-end" : ""}`}
    >
      {/* The value is what matters: the name gives way first */}
      {aggregate && <span className="min-w-0 shrink-[1000] truncate font-sans text-[10px] text-muted">{shortOf(aggregate)}</span>}
      <span className="min-w-0 truncate">{shown}</span>
      <span className={`shrink-0 text-[10px] text-faint ${aggregate ? "hidden group-hover:inline" : ""}`}>▾</span>
    </button>
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
