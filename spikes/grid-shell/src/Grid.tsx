import { useVirtualizer } from "@tanstack/react-virtual";
import { forwardRef, useCallback, useEffect, useImperativeHandle, useReducer, useRef, useState } from "react";
import { api, CursorInfo, PAGE, Row } from "./api";

const ROW_H = 22;
const COL_W = 150;
const GUTTER_W = 72;
/** Pages kept on the JS side. The Rust cursor holds every fetched row; this
 *  is only a window onto it, so it stays bounded however far the user goes. */
const MAX_PAGES = 100;

export interface GridHandle {
  scrollToRow(index: number): void;
  /** Every row inside the viewport has its data (no placeholders). */
  viewportLoaded(): boolean;
  scrollElement(): HTMLDivElement | null;
  stats(): { fetched: number; done: boolean; cachedPages: number };
}

interface Props {
  cursor: CursorInfo;
  onProgress?: (fetched: number, done: boolean) => void;
}

export const Grid = forwardRef<GridHandle, Props>(function Grid({ cursor, onProgress }, ref) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const pages = useRef(new Map<number, Row[]>());
  const inflight = useRef(new Set<number>());
  const [, rerender] = useReducer((n: number) => n + 1, 0);
  const [fetched, setFetched] = useState(0);
  const [done, setDone] = useState(false);

  // Until the source is exhausted, one page of placeholder rows past the end
  // gives the user somewhere to scroll to, which is what pulls the next page.
  const rowCount = done ? fetched : fetched + PAGE;

  // The handle outlives renders (callers keep it), so it reads live values
  // from here rather than closing over one render's state.
  const live = useRef({ fetched, done, rowCount });
  live.current = { fetched, done, rowCount };

  const rows = useVirtualizer({
    count: rowCount,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => ROW_H,
    overscan: 20,
  });
  const cols = useVirtualizer({
    horizontal: true,
    count: cursor.columns.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => COL_W,
    overscan: 3,
    paddingStart: GUTTER_W,
  });

  const load = useCallback(
    (page: number) => {
      if (pages.current.has(page) || inflight.current.has(page)) return;
      inflight.current.add(page);
      api
        .rows(cursor.id, page * PAGE, PAGE)
        .then((chunk) => {
          pages.current.set(page, chunk.rows);
          setFetched(chunk.fetched);
          setDone(chunk.done);
          onProgress?.(chunk.fetched, chunk.done);
          rerender();
        })
        .finally(() => inflight.current.delete(page));
    },
    [cursor.id, onProgress],
  );

  const virtualRows = rows.getVirtualItems();
  const firstPage = virtualRows.length ? Math.floor(virtualRows[0].index / PAGE) : 0;
  const lastPage = virtualRows.length ? Math.floor(virtualRows[virtualRows.length - 1].index / PAGE) : 0;

  useEffect(() => {
    const cache = pages.current;
    for (let p = firstPage; p <= lastPage; p++) {
      const hit = cache.get(p);
      if (hit) {
        // Map order is the LRU order: re-inserting marks the page as recent.
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
  }, [firstPage, lastPage, load]);

  useEffect(() => {
    pages.current.clear();
    inflight.current.clear();
    setFetched(0);
    setDone(false);
    scrollRef.current?.scrollTo({ top: 0, left: 0 });
  }, [cursor.id]);

  const rowAt = (i: number): Row | undefined => pages.current.get(Math.floor(i / PAGE))?.[i % PAGE];

  useImperativeHandle(ref, () => ({
    scrollToRow: (i) => rows.scrollToIndex(i, { align: "start" }),
    viewportLoaded: () => {
      const el = scrollRef.current;
      const { fetched, done, rowCount } = live.current;
      if (!el) return false;
      // Row i sits at top = (i + 1) * ROW_H: the header takes the first slot.
      const first = Math.max(0, Math.floor(el.scrollTop / ROW_H) - 1);
      const last = Math.min(rowCount - 1, Math.ceil((el.scrollTop + el.clientHeight) / ROW_H));
      for (let i = first; i <= last; i++) {
        if (i >= fetched && done) break;
        if (!rowAt(i)) return false;
      }
      return true;
    },
    scrollElement: () => scrollRef.current,
    stats: () => ({ fetched: live.current.fetched, done: live.current.done, cachedPages: pages.current.size }),
  }), []);

  const virtualCols = cols.getVirtualItems();
  const width = cols.getTotalSize();

  return (
    <div className="grid" ref={scrollRef}>
      <div style={{ height: rows.getTotalSize() + ROW_H, width, position: "relative" }}>
        <div className="header" style={{ width }}>
          <div className="gutter">#</div>
          {virtualCols.map((c) => (
            <div key={c.key} className="cell head" style={{ left: c.start, width: c.size }}>
              {cursor.columns[c.index]}
            </div>
          ))}
        </div>
        {virtualRows.map((r) => {
          const row = rowAt(r.index);
          return (
            <div key={r.key} className="row" style={{ top: r.start + ROW_H, width }}>
              <div className="gutter">{r.index + 1}</div>
              {virtualCols.map((c) => {
                const v = row?.[c.index];
                return (
                  <div key={c.key} className="cell" style={{ left: c.start, width: c.size }}>
                    {row === undefined ? <span className="pending" /> : v === null ? <span className="null">&lt;null&gt;</span> : v}
                  </div>
                );
              })}
            </div>
          );
        })}
      </div>
    </div>
  );
});
