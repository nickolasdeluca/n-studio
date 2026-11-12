// A select's rows. The backend holds every fetched row; the grid asks for
// the pages in view and keeps a bounded few, so a million rows cost the UI
// what a screenful does. Reaching the end of what was fetched fetches more.
//
// With `edit`, cells take new values, rows are marked to be deleted and new
// rows are added after the fetched ones: all of it kept by whoever owns the
// grid until it is applied, and shown over the rows as they were read.
import { useVirtualizer } from "@tanstack/react-virtual";
import { useCallback, useEffect, useImperativeHandle, useMemo, useReducer, useRef, useState, type Ref } from "react";
import { api, type Cell, type ColumnInfo, type ColumnKind, type ResultView } from "../api";
import { PAGE } from "../editors";
import { t } from "../i18n";
import {
  addInserted,
  removeInserted,
  rowId,
  setCell,
  setInserted,
  toggleDeleted,
  type RowEdits,
} from "../rowEdits";

const ROW_H = 22;
const GUTTER_W = 56;
/** Pages kept on this side, least recently viewed dropped first */
const MAX_PAGES = 60;
/** Fetch more when the view comes this close to the last fetched row */
const AHEAD = 40;

export interface GridCursor {
  row: number;
  col: number;
}

/** How the grid's rows can be changed, and what was changed so far */
export interface GridEdit {
  /** The columns that tell a row from every other, by index */
  key: number[];
  /** By column: whether a value can be typed into it */
  editable: boolean[];
  edits: RowEdits;
  onEdits(edits: RowEdits): void;
}

/** What a toolbar does to the row or the cell the cursor is on */
export interface GridHandle {
  insertRow(): void;
  deleteRow(): void;
  setNull(): void;
}

/** What a row is to the edits: a new one, or a fetched one by its key */
type RowState =
  | { inserted: number }
  | { inserted?: undefined; key: Cell[]; cells: Cell[]; set: Record<string, Cell> | undefined; deleted: boolean };

interface Props {
  result: ResultView;
  columns: ColumnInfo[];
  cursor: GridCursor;
  onCursor(cursor: GridCursor): void;
  onFetchMore(max: number): Promise<void>;
  /** A header was clicked: sort by that column, or stop sorting */
  onSort(col: number): void;
  edit?: GridEdit;
  ref?: Ref<GridHandle>;
}

const numeric = (k: ColumnKind) => k === "integer" || k === "decimal" || k === "float";

/** A first width from what the column holds, before the user drags it */
function initialWidth(c: ColumnInfo): number {
  const byKind: Partial<Record<ColumnKind, number>> = {
    integer: 90,
    decimal: 120,
    float: 120,
    date: 100,
    time: 100,
    timestamp: 180,
    boolean: 70,
  };
  const name = c.name.length * 7.5 + 24;
  const chars = /\((\d+)\)/.exec(c.typeName);
  const content = byKind[c.kind] ?? (chars ? Math.min(Number(chars[1]) * 7.5 + 16, 320) : 200);
  return Math.round(Math.max(60, Math.min(Math.max(name, content), 360)));
}

export function ResultGrid({ result, columns, cursor, onCursor, onFetchMore, onSort, edit, ref }: Props) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const pages = useRef(new Map<number, Cell[][]>());
  const inflight = useRef(new Set<number>());
  const fetching = useRef(false);
  const [, rerender] = useReducer((n: number) => n + 1, 0);
  const [widths, setWidths] = useState<number[]>(() => columns.map(initialWidth));
  /** The cell being typed into; `first`: it began with a key, not with all selected */
  const [typing, setTyping] = useState<{ row: number; col: number; text: string; initial: string; first: boolean } | null>(null);
  // Set when the keyboard ended the typing, so that losing focus does not end it again
  const ended = useRef(false);
  const inserted = edit?.edits.inserted ?? [];
  const total = result.rows + inserted.length;
  const names = useMemo(() => columns.map((c) => c.name), [columns]);

  // A new result starts over
  useEffect(() => {
    pages.current.clear();
    inflight.current.clear();
    setWidths(columns.map(initialWidth));
    scrollRef.current?.scrollTo({ top: 0, left: 0 });
  }, [result.id, columns]);

  // Sorted another way: every page holds other rows now
  const order = result.sort ? `${result.sort.column}:${result.sort.descending}` : "";
  useEffect(() => {
    pages.current.clear();
    rerender();
  }, [order]);

  // More rows arrived: the page that was the last one may have grown
  useEffect(() => {
    for (const [p, rows] of pages.current) if (rows.length < PAGE) pages.current.delete(p);
    rerender();
  }, [result.rows]);

  const rows = useVirtualizer({
    count: total,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => ROW_H,
    overscan: 20,
    // The sticky header takes the first row's height
    paddingStart: ROW_H,
    scrollPaddingStart: ROW_H,
  });
  const cols = useVirtualizer({
    horizontal: true,
    count: columns.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: (i) => widths[i] ?? 120,
    overscan: 4,
    paddingStart: GUTTER_W,
  });
  useEffect(() => {
    cols.measure();
  }, [widths, cols]);

  const load = useCallback(
    (page: number) => {
      if (pages.current.has(page) || inflight.current.has(page)) return;
      inflight.current.add(page);
      const id = result.id;
      api
        .resultRows(id, page * PAGE, PAGE)
        .then((chunk) => {
          if (id !== result.id) return;
          pages.current.set(page, chunk);
          rerender();
        })
        .catch(() => {})
        .finally(() => inflight.current.delete(page));
    },
    [result.id],
  );

  const virtualRows = rows.getVirtualItems();
  const first = virtualRows[0]?.index ?? 0;
  const last = virtualRows[virtualRows.length - 1]?.index ?? 0;
  const firstPage = Math.floor(first / PAGE);
  const lastPage = Math.floor(last / PAGE);

  useEffect(() => {
    const cache = pages.current;
    for (let p = firstPage; p <= lastPage; p++) {
      const hit = cache.get(p);
      if (hit) {
        // Map order is the LRU order: re-inserting marks the page as recent
        cache.delete(p);
        cache.set(p, hit);
      } else {
        load(p);
      }
    }
    for (const p of cache.keys()) {
      if (cache.size <= MAX_PAGES) break;
      if (p < firstPage || p > lastPage) cache.delete(p);
    }
  }, [firstPage, lastPage, load, result.rows]);

  // Near the end of what was fetched: fetch the next pages
  useEffect(() => {
    if (result.eof || fetching.current || last < result.rows - AHEAD) return;
    fetching.current = true;
    void onFetchMore(PAGE * 5).finally(() => {
      fetching.current = false;
    });
  }, [last, result.rows, result.eof, onFetchMore]);

  const cellAt = (r: number, c: number): Cell | undefined => pages.current.get(Math.floor(r / PAGE))?.[r % PAGE]?.[c];

  // Keep the cursor in view as it moves, and only then: more rows arriving
  // must not pull the view back to it
  const live = useRef({ rows, cols, count: total });
  live.current = { rows, cols, count: total };
  useEffect(() => {
    const { rows, cols, count } = live.current;
    if (count === 0) return;
    rows.scrollToIndex(cursor.row, { align: "auto" });
    cols.scrollToIndex(cursor.col, { align: "auto" });
  }, [cursor.row, cursor.col]);

  const move = (row: number, col: number) => {
    const maxRow = Math.max(0, total - 1);
    onCursor({ row: Math.max(0, Math.min(row, maxRow)), col: Math.max(0, Math.min(col, columns.length - 1)) });
  };

  const rowState = (r: number): RowState | null => {
    if (!edit) return null;
    if (r >= result.rows) return r < total ? { inserted: r - result.rows } : null;
    const cells = pages.current.get(Math.floor(r / PAGE))?.[r % PAGE];
    if (!cells) return null;
    const key = edit.key.map((k) => cells[k]);
    const id = rowId(key);
    return { key, cells, set: edit.edits.changed[id]?.set, deleted: id in edit.edits.deleted };
  };
  /** A cell as it will be once the edits are applied; `undefined` for a
   *  new row's cell nothing was typed into */
  const shown = (state: RowState | null, r: number, c: number): { value: Cell | undefined; dirty: boolean } => {
    if (state?.inserted !== undefined) {
      const row = inserted[state.inserted];
      return { value: row?.[names[c]], dirty: row !== undefined && names[c] in row };
    }
    if (state?.set && names[c] in state.set) return { value: state.set[names[c]], dirty: true };
    return { value: cellAt(r, c), dirty: false };
  };
  const put = (r: number, c: number, value: Cell) => {
    const state = rowState(r);
    if (!edit || !state || !edit.editable[c]) return;
    if (state.inserted !== undefined) edit.onEdits(setInserted(edit.edits, state.inserted, names[c], value));
    else if (!state.deleted) edit.onEdits(setCell(edit.edits, state.key, names[c], value, state.cells[c]));
  };

  const startTyping = (first?: string) => {
    const { row, col } = cursor;
    const state = rowState(row);
    if (!edit?.editable[col] || !state || (state.inserted === undefined && state.deleted)) return;
    const initial = shown(state, row, col).value ?? "";
    ended.current = false;
    setTyping({ row, col, text: first ?? initial, initial, first: first !== undefined });
  };
  const endTyping = (keep: boolean) => {
    if (!typing) return;
    ended.current = true;
    setTyping(null);
    scrollRef.current?.focus();
    if (!keep || typing.text === typing.initial) return;
    // Only text can hold nothing and not be null
    const empty = typing.text.trim() === "" && columns[typing.col].kind !== "text";
    put(typing.row, typing.col, empty ? null : typing.text);
  };

  const handle: GridHandle = {
    insertRow() {
      if (!edit) return;
      edit.onEdits(addInserted(edit.edits));
      const col = edit.editable[cursor.col] ? cursor.col : Math.max(0, edit.editable.indexOf(true));
      onCursor({ row: total, col });
      scrollRef.current?.focus();
    },
    deleteRow() {
      const state = rowState(cursor.row);
      if (!edit || !state) return;
      if (state.inserted !== undefined) edit.onEdits(removeInserted(edit.edits, state.inserted));
      else edit.onEdits(toggleDeleted(edit.edits, state.key));
      scrollRef.current?.focus();
    },
    setNull() {
      put(cursor.row, cursor.col, null);
      scrollRef.current?.focus();
    },
  };
  useImperativeHandle(ref, () => handle);

  const onKeyDown = (e: React.KeyboardEvent) => {
    const page = Math.max(1, Math.floor((scrollRef.current?.clientHeight ?? 200) / ROW_H) - 2);
    const mod = e.ctrlKey || e.metaKey;
    const { row, col } = cursor;
    const keys: Record<string, () => void> = {
      ArrowDown: () => move(row + 1, col),
      ArrowUp: () => move(row - 1, col),
      ArrowRight: () => move(row, col + 1),
      ArrowLeft: () => move(row, col - 1),
      PageDown: () => move(row + page, col),
      PageUp: () => move(row - page, col),
      Home: () => (mod ? move(0, col) : move(row, 0)),
      End: () => (mod ? move(total - 1, col) : move(row, columns.length - 1)),
    };
    if (keys[e.key]) {
      e.preventDefault();
      keys[e.key]();
    } else if (mod && e.key.toLowerCase() === "c") {
      e.preventDefault();
      const v = shown(rowState(row), row, col).value;
      if (v !== undefined) void navigator.clipboard.writeText(v ?? "");
    } else if (edit) {
      const erase = e.key === "Delete" || e.key === "Backspace";
      const act =
        e.key === "Enter" || e.key === "F2"
          ? () => startTyping()
          : e.key === "Insert"
            ? handle.insertRow
            : erase && mod
              ? handle.deleteRow
              : erase && e.shiftKey
                ? handle.setNull
                : e.key.length === 1 && !mod && !e.altKey
                  ? () => startTyping(e.key)
                  : null;
      if (!act) return;
      e.preventDefault();
      act();
    }
  };

  const startResize = (index: number, e: React.MouseEvent) => {
    e.preventDefault();
    e.stopPropagation();
    const x0 = e.clientX;
    const w0 = widths[index];
    const onMove = (m: MouseEvent) =>
      setWidths((ws) => ws.map((w, i) => (i === index ? Math.max(40, w0 + m.clientX - x0) : w)));
    const onUp = () => {
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
    };
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
  };

  const virtualCols = cols.getVirtualItems();
  const width = cols.getTotalSize();
  const align = useMemo(() => columns.map((c) => (numeric(c.kind) ? "text-right" : "")), [columns]);
  // Blob cells hold a marker, (MEMO) or (BLOB): the viewer reads the content
  const blob = useMemo(() => columns.map((c) => c.kind === "blob" || c.kind === "text_blob"), [columns]);

  return (
    <div
      ref={scrollRef}
      tabIndex={0}
      onKeyDown={onKeyDown}
      className="scroll-thin relative h-full overflow-auto bg-panel font-mono text-[12px] outline-none"
      role="grid"
      aria-rowcount={total}
      aria-colcount={columns.length}
    >
      <div style={{ height: rows.getTotalSize(), width, position: "relative" }}>
        <div className="sticky top-0 z-20 flex border-b border-line bg-panel-2 font-sans" style={{ height: ROW_H, width }}>
          <div className="sticky left-0 z-10 shrink-0 border-r border-line bg-panel-2" style={{ width: GUTTER_W }} />
          {virtualCols.map((c) => {
            const col = columns[c.index];
            const sorted = result.sort?.column === c.index ? result.sort : null;
            const hint = blob[c.index] ? t("Blob columns cannot be sorted") : t("Click to sort");
            return (
              <div
                key={c.key}
                role="columnheader"
                aria-sort={sorted ? (sorted.descending ? "descending" : "ascending") : undefined}
                title={`${col.name}\n${col.typeName}${col.relation ? `\n${col.relation}.${col.field}` : ""}\n${hint}`}
                onClick={() => !blob[c.index] && onSort(c.index)}
                className={`absolute top-0 flex h-full items-center gap-1 border-r border-line px-1.5 font-semibold text-muted ${
                  c.index === cursor.col ? "text-fg" : ""
                } ${align[c.index] ? "justify-end" : ""} ${blob[c.index] ? "" : "cursor-pointer hover:bg-hover"}`}
                style={{ left: c.start, width: c.size }}
              >
                <span className="truncate">{col.name}</span>
                {sorted && <span className="shrink-0 text-accent">{sorted.descending ? "▼" : "▲"}</span>}
                <span
                  className="absolute top-0 right-[-3px] z-10 h-full w-[6px] cursor-col-resize"
                  onMouseDown={(e) => startResize(c.index, e)}
                  onClick={(e) => e.stopPropagation()}
                />
              </div>
            );
          })}
        </div>
        {virtualRows.map((r) => {
          const state = rowState(r.index);
          const added = state?.inserted !== undefined;
          const loaded = added || pages.current.has(Math.floor(r.index / PAGE));
          const current = r.index === cursor.row;
          const deleted = !!state && !added && state.deleted;
          const changed = !!state && !added && !!state.set;
          return (
            <div
              key={r.key}
              role="row"
              className={`absolute left-0 flex border-b border-line/60 ${
                current ? "bg-selected-inactive" : added ? "bg-ok/5" : deleted ? "bg-danger/5" : ""
              } ${deleted ? "text-faint line-through" : ""}`}
              style={{ top: r.start, height: ROW_H, width }}
            >
              <div
                className={`sticky left-0 z-10 shrink-0 border-r border-line px-1.5 text-right text-faint ${
                  current ? "bg-selected text-fg" : "bg-panel-2"
                }`}
                style={{ width: GUTTER_W, lineHeight: `${ROW_H - 1}px` }}
              >
                {current ? "▸ " : ""}
                {added ? (
                  <span className="text-ok" title={t("A new row, not in the table yet")}>
                    +
                  </span>
                ) : (
                  <>
                    {(changed || deleted) && (
                      <span
                        className={`mr-0.5 ${deleted ? "text-danger" : "text-accent"}`}
                        title={deleted ? t("To be deleted") : t("Changed, not applied")}
                      >
                        {deleted ? "×" : "•"}
                      </span>
                    )}
                    {r.index + 1}
                  </>
                )}
              </div>
              {virtualCols.map((c) => {
                const { value: v, dirty } = loaded ? shown(state, r.index, c.index) : { value: undefined, dirty: false };
                const focused = current && c.index === cursor.col;
                if (typing && typing.row === r.index && typing.col === c.index) {
                  return (
                    <input
                      key={c.key}
                      ref={(el) => {
                        if (!el || document.activeElement === el) return;
                        el.focus();
                        if (!typing.first) el.select();
                      }}
                      value={typing.text}
                      aria-label={columns[c.index].name}
                      spellCheck={false}
                      autoComplete="off"
                      onChange={(e) => setTyping({ ...typing, text: e.target.value })}
                      onBlur={() => !ended.current && endTyping(true)}
                      onKeyDown={(e) => {
                        // The grid's keys are not for what is being typed
                        e.stopPropagation();
                        if (e.key === "Escape") endTyping(false);
                        else if (e.key === "Enter" || e.key === "Tab") {
                          e.preventDefault();
                          endTyping(true);
                          if (e.key === "Tab") move(r.index, c.index + (e.shiftKey ? -1 : 1));
                        }
                      }}
                      className={`absolute top-0 z-10 h-full border-0 bg-panel px-1.5 outline outline-2 -outline-offset-2 outline-accent ${align[c.index]}`}
                      style={{ left: c.start, width: c.size }}
                    />
                  );
                }
                return (
                  <div
                    key={c.key}
                    role="gridcell"
                    onMouseDown={() => move(r.index, c.index)}
                    onDoubleClick={() => startTyping()}
                    className={`absolute top-0 truncate border-r border-line/60 px-1.5 ${align[c.index]} ${
                      focused ? "bg-selected outline outline-1 -outline-offset-1 outline-accent" : dirty && !added ? "bg-accent/10" : ""
                    }`}
                    style={{ left: c.start, width: c.size, lineHeight: `${ROW_H - 1}px` }}
                  >
                    {!loaded ? (
                      <span className="inline-block h-2 w-10 rounded bg-hover align-middle" />
                    ) : v === undefined ? (
                      added && <span className="text-faint italic">{t("<default>")}</span>
                    ) : v === null ? (
                      <span className="text-faint italic">{t("<null>")}</span>
                    ) : blob[c.index] ? (
                      <span className="text-muted">{v}</span>
                    ) : (
                      v
                    )}
                  </div>
                );
              })}
            </div>
          );
        })}
      </div>
    </div>
  );
}
