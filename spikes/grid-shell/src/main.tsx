import { StrictMode, useCallback, useEffect, useRef, useState } from "react";
import { createRoot } from "react-dom/client";
import { api, CursorInfo, PerfReport } from "./api";
import { runBench } from "./bench";
import { Grid, GridHandle } from "./Grid";
import "./style.css";

function App() {
  const [cursor, setCursor] = useState<CursorInfo | null>(null);
  const [status, setStatus] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [perf, setPerf] = useState<PerfReport | null>(null);
  const [sql, setSql] = useState("select * from rdb$relations");
  const [synthRows, setSynthRows] = useState(1_000_000);
  const [synthCols, setSynthCols] = useState(6);
  const grid = useRef<GridHandle>(null);
  const gridReady = useRef<((g: GridHandle) => void) | null>(null);

  const show = useCallback(async (next: CursorInfo) => {
    setCursor((prev) => {
      if (prev) void api.closeCursor(prev.id);
      return next;
    });
    setPerf(null);
    setError(null);
  }, []);

  useEffect(() => {
    if (cursor && grid.current && gridReady.current) {
      gridReady.current(grid.current);
      gridReady.current = null;
    }
  }, [cursor]);

  useEffect(() => {
    void api.benchConfig().then((cfg) => {
      if (!cfg) return;
      setStatus(`bench: ${cfg}`);
      void runBench(cfg, (c) => {
        const ready = new Promise<GridHandle>((r) => (gridReady.current = r));
        void show(c);
        return ready;
      });
    });
  }, [show]);

  const run = async (open: () => Promise<CursorInfo>) => {
    try {
      const t = performance.now();
      const c = await open();
      setStatus(`opened in ${(performance.now() - t).toFixed(0)} ms`);
      await show(c);
    } catch (e) {
      setError(String(e));
    }
  };

  const onProgress = useCallback((fetched: number, done: boolean) => {
    setStatus(`${fetched.toLocaleString()} rows fetched${done ? " (all)" : ""}`);
  }, []);

  return (
    <div className="app">
      <div className="toolbar">
        <textarea value={sql} onChange={(e) => setSql(e.target.value)} rows={3} spellCheck={false} />
        <div className="actions">
          <button onClick={() => run(() => api.openQuery(sql))}>Run (F9)</button>
          <button disabled={!cursor} onClick={async () => cursor && setPerf(await api.perf(cursor.id))}>
            Performance
          </button>
          <span className="sep" />
          <input type="number" value={synthRows} onChange={(e) => setSynthRows(+e.target.value)} />
          ×
          <input type="number" value={synthCols} onChange={(e) => setSynthCols(+e.target.value)} />
          <button onClick={() => run(() => api.openSynthetic(synthRows, synthCols))}>Synthetic</button>
        </div>
      </div>
      {error && <pre className="error">{error}</pre>}
      <div className="main">{cursor && <Grid ref={grid} cursor={cursor} onProgress={onProgress} />}</div>
      {perf && <Perf report={perf} />}
      <div className="status">{status}</div>
    </div>
  );
}

function Perf({ report: p }: { report: PerfReport }) {
  return (
    <div className="perf">
      <pre>{p.plan?.trim() ?? "<no plan>"}</pre>
      <table>
        <tbody>
          <tr><td>Prepare</td><td>{p.prepare_ms.toFixed(1)} ms</td><td>Reads</td><td>{p.reads}</td><td>Current memory</td><td>{p.current_memory.toLocaleString()}</td></tr>
          <tr><td>Execute</td><td>{p.execute_ms.toFixed(1)} ms</td><td>Writes</td><td>{p.writes}</td><td>Max memory</td><td>{p.max_memory.toLocaleString()}</td></tr>
          <tr><td>Fetch ({p.rows})</td><td>{p.fetch_ms.toFixed(1)} ms</td><td>Fetches</td><td>{p.fetches}</td><td>Buffers</td><td>{p.buffers}</td></tr>
          <tr><td>Avg fetch</td><td>{p.rows ? (p.fetch_ms / p.rows).toFixed(3) : "–"} ms</td><td>Marks</td><td>{p.marks}</td><td /><td /></tr>
        </tbody>
      </table>
      <table>
        <thead><tr><th>Table</th><th>Indexed reads</th><th>Non-indexed reads</th></tr></thead>
        <tbody>
          {p.tables.map(([name, idx, seq]) => (
            <tr key={name}><td>{name}</td><td>{idx}</td><td>{seq}</td></tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
