import { api, CursorInfo, PAGE } from "./api";
import type { GridHandle } from "./Grid";

const frame = () => new Promise<number>((r) => requestAnimationFrame(() => r(performance.now())));

// macOS stops delivering frames to a window that is covered by another one,
// so a sample taken across an occlusion measures the user's desktop, not the
// grid (one such "frame" lasted 272 s). Samples spanning a visibility change
// are dropped and counted instead.
let visibilityChanges = 0;
document.addEventListener("visibilitychange", () => visibilityChanges++);
const STALL_MS = 1000;

/** Keeps `ms` unless the page was hidden meanwhile or frames stopped arriving. */
function keep(samples: number[], ms: number, changesBefore: number, dropped: { n: number }) {
  if (visibilityChanges !== changesBefore || ms > STALL_MS) dropped.n++;
  else samples.push(ms);
}

function pct(xs: number[], p: number) {
  if (!xs.length) return 0;
  const s = [...xs].sort((a, b) => a - b);
  return +s[Math.min(s.length - 1, Math.floor((p / 100) * s.length))].toFixed(2);
}

const summary = (xs: number[]) => ({ n: xs.length, p50: pct(xs, 50), p95: pct(xs, 95), max: pct(xs, 100) });

async function until(cond: () => boolean, timeoutMs = 10_000) {
  const t0 = performance.now();
  while (!cond()) {
    if (performance.now() - t0 > timeoutMs) throw new Error("timed out waiting for the grid");
    await frame();
  }
}

/**
 * Unattended measurement. `config` is `synthetic:<rows>:<cols>` or `sql:<select>`.
 * `show` hands the cursor to the grid and resolves with its handle.
 */
export async function runBench(config: string, show: (c: CursorInfo) => Promise<GridHandle>) {
  const report: Record<string, unknown> = { config };
  let grid: GridHandle | null = null;
  try {
    const [kind, ...rest] = config.split(":");
    const tOpen = performance.now();
    const cursor =
      kind === "synthetic"
        ? await api.openSynthetic(Number(rest[0]), Number(rest[1]))
        : await api.openQuery(rest.join(":"));
    report.open_ms = +(performance.now() - tOpen).toFixed(1);
    report.columns = cursor.columns.length;

    // 1. Stream everything into the Rust store over IPC, page by page — what
    //    holding End down in the grid amounts to, minus the rendering.
    const t0 = performance.now();
    let start = 0;
    let firstPageMs = 0;
    let sampledBytes = 0;
    let sampledPages = 0;
    for (let page = 0; ; page++) {
      const chunk = await api.rows(cursor.id, start, PAGE);
      if (page === 0) firstPageMs = performance.now() - t0;
      if (page % 50 === 0) {
        sampledBytes += JSON.stringify(chunk.rows).length;
        sampledPages++;
      }
      start += chunk.rows.length;
      if (chunk.done || chunk.rows.length === 0) break;
    }
    const streamMs = performance.now() - t0;
    report.stream = {
      rows: start,
      ms: +streamMs.toFixed(0),
      rows_per_s: Math.round(start / (streamMs / 1000)),
      first_page_ms: +firstPageMs.toFixed(1),
      json_bytes_per_page: Math.round(sampledBytes / Math.max(1, sampledPages)),
    };

    grid = await show(cursor);
    const g = grid;
    await until(() => g.viewportLoaded() && g.stats().fetched > 0);
    const total = g.stats().fetched;

    // 2. Random jumps across the whole result: time from setting the scroll
    //    position to the first frame with every visible row filled in.
    const jumps: number[] = [];
    const droppedJumps = { n: 0 };
    for (let i = 0; i < 100; i++) {
      const target = Math.floor(Math.random() * Math.max(1, total - 60));
      const changes = visibilityChanges;
      const t = performance.now();
      g.scrollToRow(target);
      await frame();
      await until(() => g.viewportLoaded(), 600_000);
      keep(jumps, performance.now() - t, changes, droppedJumps);
    }
    report.jump_ms = { ...summary(jumps), dropped: droppedJumps.n };

    // 3. Continuous scrolling, 3 rows per frame, from the middle: frame times
    //    and frames that showed placeholders instead of data.
    const el = g.scrollElement()!;
    g.scrollToRow(Math.floor(total / 2));
    await until(() => g.viewportLoaded(), 600_000);
    const frames: number[] = [];
    const droppedFrames = { n: 0 };
    let blank = 0;
    let last = await frame();
    for (let i = 0; i < 600; i++) {
      const changes = visibilityChanges;
      el.scrollTop += 66;
      const now = await frame();
      keep(frames, now - last, changes, droppedFrames);
      last = now;
      if (!g.viewportLoaded()) blank++;
    }
    report.scroll_frame_ms = {
      ...summary(frames),
      over_20ms: frames.filter((f) => f > 20).length,
      blank_frames: blank,
      dropped: droppedFrames.n,
    };
    report.js_cached_pages = g.stats().cachedPages;

    if (kind === "sql") report.perf = await api.perf(cursor.id);
  } catch (e) {
    report.error = String(e);
    report.diag = {
      grid: grid?.stats() ?? null,
      scroll_height: grid?.scrollElement()?.scrollHeight ?? null,
      client_height: grid?.scrollElement()?.clientHeight ?? null,
      visibility: document.visibilityState,
      raf_fires: await Promise.race([frame().then(() => true), new Promise((r) => setTimeout(() => r(false), 500))]),
    };
  }
  await api.benchReport(report);
}
