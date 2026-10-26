import { invoke } from "@tauri-apps/api/core";

export type Row = (string | null)[];

export interface CursorInfo {
  id: number;
  columns: string[];
}

export interface Chunk {
  start: number;
  rows: Row[];
  fetched: number;
  done: boolean;
}

export interface PerfReport {
  plan: string | null;
  explained_plan: string | null;
  prepare_ms: number;
  execute_ms: number;
  fetch_ms: number;
  rows: number;
  reads: number;
  writes: number;
  fetches: number;
  marks: number;
  current_memory: number;
  max_memory: number;
  buffers: number;
  tables: [string, number, number][];
}

/** Rows per request; matches the Rust cursor's page. */
export const PAGE = 200;

export const api = {
  openSynthetic: (rows: number, cols: number) =>
    invoke<CursorInfo>("open_synthetic", { rows, cols }),
  openQuery: (sql: string) => invoke<CursorInfo>("open_query", { sql }),
  rows: (id: number, start: number, count: number) =>
    invoke<Chunk>("rows", { id, start, count }),
  perf: (id: number) => invoke<PerfReport | null>("perf", { id }),
  closeCursor: (id: number) => invoke<void>("close_cursor", { id }),
  benchConfig: () => invoke<string | null>("bench_config"),
  benchReport: (report: object) => invoke<void>("bench_report", { report }),
};
