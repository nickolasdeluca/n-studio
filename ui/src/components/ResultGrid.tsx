// A select's rows. The backend holds every fetched row; the grid asks for
// the pages in view and keeps a bounded few, so a million rows cost the UI
// what a screenful does. Reaching the end of what was fetched fetches more.
import { useVirtualizer } from "@tanstack/react-virtual";
import { useCallback, useEffect, useMemo, useReducer, useRef, useState } from "react";
import { api, type Cell, type ColumnInfo, type ColumnKind, type ResultView } from "../api";
import { PAGE } from "../editors";
import { t } from "../i18n";

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

interface Props {
  result: ResultView;
  columns: ColumnInfo[];
  cursor: GridCursor;
  onCursor(cursor: GridCursor): void;
  onFetchMore(max: number): Promise<void>;
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

export function ResultGrid({ result, columns, cursor, onCursor, onFetchMore }: Props) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const pages = useRef(new Map<number, Cell[][]>());
  const inflight = useRef(new Set<number>());
  const fetching = useRef(false);
  const [, rerender] = useReducer((n: number) => n + 1, 0);
  const [widths, setWidths] = useState<number[]>(() => columns.map(initialWidth));

  // A new result starts over
  useEffect(() => {
    pages.current.clear();
    inflight.current.clear();
    setWidths(columns.map(initialWidth));
    scrollRef.current?.scrollTo({ top: 0, left: 0 });
  }, [result.id, columns]);

  // More rows arrived: the page that was the last one may have grown
  useEffect(() => {
    for (const [p, rows] of pages.current) if (rows.length < PAGE) pages.current.delete(p);
    rerender();
  }, [result.rows]);

  const rows = useVirtualizer({
    count: result.rows,
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
  const live = useRef({ rows, cols, count: result.rows });
  live.current = { rows, cols, count: result.rows };
  useEffect(() => {
    const { rows, cols, count } = live.current;
    if (count === 0) return;
    rows.scrollToIndex(cursor.row, { align: "auto" });
    cols.scrollToIndex(cursor.col, { align: "auto" });
  }, [cursor.row, cursor.col]);

  const move = (row: number, col: number) => {
    const maxRow = Math.max(0, result.rows - 1);
    onCursor({ row: Math.max(0, Math.min(row, maxRow)), col: Math.max(0, Math.min(col, columns.length - 1)) });
  };

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
      End: () => (mod ? move(result.rows - 1, col) : move(row, columns.length - 1)),
    };
    if (keys[e.key]) {
      e.preventDefault();
      keys[e.key]();
    } else if (mod && e.key.toLowerCase() === "c") {
      e.preventDefault();
      const v = cellAt(row, col);
      if (v !== undefined) void navigator.clipboard.writeText(v ?? "");
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

  return (
    <div
      ref={scrollRef}
      tabIndex={0}
      onKeyDown={onKeyDown}
      className="scroll-thin relative h-full overflow-auto bg-panel font-mono text-[12px] outline-none"
      role="grid"
      aria-rowcount={result.rows}
      aria-colcount={columns.length}
    >
      <div style={{ height: rows.getTotalSize(), width, position: "relative" }}>
        <div className="sticky top-0 z-20 flex border-b border-line bg-panel-2 font-sans" style={{ height: ROW_H, width }}>
          <div className="sticky left-0 z-10 shrink-0 border-r border-line bg-panel-2" style={{ width: GUTTER_W }} />
          {virtualCols.map((c) => {
            const col = columns[c.index];
            return (
              <div
                key={c.key}
                role="columnheader"
                title={`${col.name}\n${col.typeName}${col.relation ? `\n${col.relation}.${col.field}` : ""}`}
                className={`absolute top-0 flex h-full items-center border-r border-line px-1.5 font-semibold text-muted ${
                  c.index === cursor.col ? "text-fg" : ""
                } ${align[c.index] ? "justify-end" : ""}`}
                style={{ left: c.start, width: c.size }}
              >
                <span className="truncate">{col.name}</span>
                <span
                  className="absolute top-0 right-[-3px] z-10 h-full w-[6px] cursor-col-resize"
                  onMouseDown={(e) => startResize(c.index, e)}
                />
              </div>
            );
          })}
        </div>
        {virtualRows.map((r) => {
          const loaded = pages.current.has(Math.floor(r.index / PAGE));
          const current = r.index === cursor.row;
          return (
            <div
              key={r.key}
              role="row"
              className={`absolute left-0 flex border-b border-line/60 ${current ? "bg-selected-inactive" : ""}`}
              style={{ top: r.start, height: ROW_H, width }}
            >
              <div
                className={`sticky left-0 z-10 shrink-0 border-r border-line px-1.5 text-right text-faint ${
                  current ? "bg-selected text-fg" : "bg-panel-2"
                }`}
                style={{ width: GUTTER_W, lineHeight: `${ROW_H - 1}px` }}
              >
                {current ? "▸ " : ""}
                {r.index + 1}
              </div>
              {virtualCols.map((c) => {
                const v = loaded ? cellAt(r.index, c.index) : undefined;
                const focused = current && c.index === cursor.col;
                return (
                  <div
                    key={c.key}
                    role="gridcell"
                    onMouseDown={() => move(r.index, c.index)}
                    className={`absolute top-0 truncate border-r border-line/60 px-1.5 ${align[c.index]} ${
                      focused ? "bg-selected outline outline-1 -outline-offset-1 outline-accent" : ""
                    }`}
                    style={{ left: c.start, width: c.size, lineHeight: `${ROW_H - 1}px` }}
                  >
                    {!loaded ? (
                      <span className="inline-block h-2 w-10 rounded bg-hover align-middle" />
                    ) : v === null ? (
                      <span className="text-faint italic">{t("<null>")}</span>
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
