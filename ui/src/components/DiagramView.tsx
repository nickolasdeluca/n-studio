// A diagram's tab: the tables as boxes that can be dragged, the foreign
// keys as lines from the column that holds one to the table it points at.
// All of it is one SVG, so what is on screen is what Export writes.
import { save } from "@tauri-apps/plugin-dialog";
import type { IDockviewPanelProps } from "dockview-react";
import { useCallback, useEffect, useId, useMemo, useRef, useState } from "react";
import { api, errorText, type DiagramReference, type DiagramTable } from "../api";
import { HEADER_H, ROW_H, shownColumns, sizeOf, useDiagrams, type Detail, type Point } from "../diagram";
import { openObject } from "../dock";
import { t } from "../i18n";
import { useStore } from "../store";
import { useContextMenu } from "./ContextMenu";
import { TextButton } from "./ObjectViewer";

interface View {
  x: number;
  y: number;
  scale: number;
}

interface Box extends Point {
  w: number;
  h: number;
}

const MIN_SCALE = 0.08;
const MAX_SCALE = 2.5;

/** A reference's line: out of the side of each box that faces the other, at
 *  the height of the column that holds or receives it */
function path(r: DiagramReference, from: Box, to: Box, fromRow: number, toRow: number): { d: string; mid: Point } {
  const y1 = from.y + fromRow;
  const y2 = to.y + toRow;
  if (r.from === r.to) {
    // A table that references itself: a loop out of its right side
    const x = from.x + from.w;
    return { d: `M ${x} ${y1} C ${x + 46} ${y1}, ${x + 46} ${y2}, ${x} ${y2}`, mid: { x: x + 34, y: (y1 + y2) / 2 } };
  }
  const apart = to.x >= from.x + from.w || from.x >= to.x + to.w;
  let x1: number;
  let x2: number;
  let d1: number;
  let d2: number;
  if (apart) {
    const right = to.x > from.x;
    x1 = right ? from.x + from.w : from.x;
    x2 = right ? to.x : to.x + to.w;
    d1 = right ? 1 : -1;
    d2 = -d1;
  } else {
    // One above the other: both out of the same side, around
    x1 = from.x + from.w;
    x2 = to.x + to.w;
    d1 = 1;
    d2 = 1;
  }
  const reach = Math.max(36, Math.min(160, Math.abs(x2 - x1) / 2));
  const c1 = x1 + d1 * reach;
  const c2 = x2 + d2 * reach;
  return {
    d: `M ${x1} ${y1} C ${c1} ${y1}, ${c2} ${y2}, ${x2} ${y2}`,
    mid: { x: (x1 + 3 * c1 + 3 * c2 + x2) / 8, y: (y1 + y2) / 2 },
  };
}

/** How far down a box a column's line is: its row when it shows, the
 *  header otherwise */
function rowOf(table: DiagramTable, detail: Detail, column: string): number {
  const index = shownColumns(table, detail).findIndex((c) => c.name === column);
  return index < 0 ? HEADER_H / 2 : HEADER_H + 2 + index * ROW_H + ROW_H / 2;
}

export function DiagramView({ params, api: dock }: IDockviewPanelProps<{ id: string }>) {
  const panel = useDiagrams((s) => s.panels[params.id]);
  const connected = useStore((s) => (panel ? !!s.servers[panel.registration] : false));
  const showMenu = useContextMenu((m) => m.show);
  const svg = useRef<SVGSVGElement>(null);
  const [view, setView] = useState<View>({ x: 40, y: 40, scale: 1 });
  const [selected, setSelected] = useState<string | null>(null);
  const [adding, setAdding] = useState("");
  const list = useId();
  // What is being dragged: the canvas, or a table by its grip point
  const drag = useRef<{ table: string | null; dx: number; dy: number; moved: boolean } | null>(null);
  // Fitted once, when the first tables arrive
  const fitted = useRef(false);

  useEffect(() => {
    if (!panel || !connected) dock.close();
  }, [panel, connected, dock]);

  const data = panel?.data;
  const detail = panel?.detail ?? "all";
  const positions = panel?.positions;

  const boxes = useMemo(() => {
    const map = new Map<string, Box>();
    for (const table of data?.tables ?? []) {
      const at = positions?.[table.name];
      if (at) map.set(table.name, { ...at, ...sizeOf(table, detail) });
    }
    return map;
  }, [data, positions, detail]);

  const fit = useCallback(() => {
    const el = svg.current;
    if (!el || boxes.size === 0) return;
    const all = [...boxes.values()];
    const left = Math.min(...all.map((b) => b.x));
    const top = Math.min(...all.map((b) => b.y));
    const width = Math.max(...all.map((b) => b.x + b.w)) - left;
    const height = Math.max(...all.map((b) => b.y + b.h)) - top;
    const { clientWidth, clientHeight } = el;
    if (clientWidth === 0 || clientHeight === 0) return;
    const scale = Math.max(MIN_SCALE, Math.min(1, (clientWidth - 60) / width, (clientHeight - 60) / height));
    setView({ scale, x: (clientWidth - width * scale) / 2 - left * scale, y: (clientHeight - height * scale) / 2 - top * scale });
  }, [boxes]);

  useEffect(() => {
    if (fitted.current || boxes.size === 0) return;
    fitted.current = true;
    fit();
  }, [boxes, fit]);

  // Zooms about the pointer. A native listener: React's wheel is passive
  useEffect(() => {
    const el = svg.current;
    if (!el) return;
    const onWheel = (e: WheelEvent) => {
      e.preventDefault();
      const rect = el.getBoundingClientRect();
      const px = e.clientX - rect.left;
      const py = e.clientY - rect.top;
      setView((v) => {
        const scale = Math.max(MIN_SCALE, Math.min(MAX_SCALE, v.scale * Math.exp(-e.deltaY * 0.0015)));
        const k = scale / v.scale;
        return { scale, x: px - (px - v.x) * k, y: py - (py - v.y) * k };
      });
    };
    el.addEventListener("wheel", onWheel, { passive: false });
    return () => el.removeEventListener("wheel", onWheel);
  }, [panel === undefined]);

  if (!panel) return <div className="h-full bg-panel" />;
  const diagrams = useDiagrams.getState();
  const tables = data?.tables ?? [];
  const missing = (data?.all ?? []).filter((name) => !boxes.has(name));

  const onPointerDown = (e: React.PointerEvent, table: string | null) => {
    if (e.button !== 0) return;
    e.stopPropagation();
    (e.currentTarget as Element).setPointerCapture(e.pointerId);
    const at = table ? boxes.get(table) : null;
    drag.current = at
      ? { table, dx: e.clientX / view.scale - at.x, dy: e.clientY / view.scale - at.y, moved: false }
      : { table: null, dx: e.clientX - view.x, dy: e.clientY - view.y, moved: false };
    setSelected(table);
  };
  const onPointerMove = (e: React.PointerEvent) => {
    const d = drag.current;
    if (!d) return;
    d.moved = true;
    if (d.table) diagrams.move(panel.id, d.table, { x: Math.round(e.clientX / view.scale - d.dx), y: Math.round(e.clientY / view.scale - d.dy) });
    else setView((v) => ({ ...v, x: e.clientX - d.dx, y: e.clientY - d.dy }));
  };
  const onPointerUp = () => {
    drag.current = null;
  };

  const menu = (e: React.MouseEvent, table: DiagramTable) => {
    e.preventDefault();
    e.stopPropagation();
    setSelected(table.name);
    showMenu(e.clientX, e.clientY, [
      { label: t("Open {name}", { name: table.name }), icon: "open", onSelect: () => openObject(panel.registration, "table", table.name) },
      "separator",
      {
        label: t("Add Related Tables ({n})", { n: table.hidden.length }),
        icon: "plus",
        disabled: table.hidden.length === 0,
        onSelect: () => void diagrams.add(panel.id, table.hidden),
      },
      { label: t("Remove from Diagram"), onSelect: () => void diagrams.remove(panel.id, table.name) },
    ]);
  };

  const add = () => {
    const name = adding.trim().toUpperCase();
    const found = missing.find((table) => table === adding.trim()) ?? missing.find((table) => table.toUpperCase() === name);
    if (!found) return;
    setAdding("");
    void diagrams.add(panel.id, [found]);
  };

  const exportSvg = async () => {
    const el = svg.current;
    if (!el || boxes.size === 0) return;
    try {
      const path = await save({ defaultPath: "diagram.svg", filters: [{ name: "SVG", extensions: ["svg"] }] });
      if (!path) return;
      await api.writeTextFile(path, standalone(el, [...boxes.values()]));
    } catch (e) {
      useStore.getState().setNotice(errorText(e));
    }
  };

  const related = new Set<string>();
  if (selected) {
    for (const r of data?.references ?? []) {
      if (r.from === selected) related.add(r.to);
      if (r.to === selected) related.add(r.from);
    }
  }

  return (
    <div className="flex h-full flex-col bg-panel">
      <div className="flex h-8 shrink-0 items-center gap-1 border-b border-line bg-panel-2 px-2">
        <input
          className="h-6 w-52 rounded border border-line bg-panel px-1.5 text-fg outline-none focus:border-accent"
          placeholder={t("Add a table…")}
          list={list}
          value={adding}
          spellCheck={false}
          onChange={(e) => setAdding(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && add()}
        />
        <datalist id={list}>
          {missing.slice(0, 500).map((name) => (
            <option key={name} value={name} />
          ))}
        </datalist>
        <label className="ml-2 flex items-center gap-1.5 text-muted">
          {t("Columns")}
          <select
            className="h-6 rounded border border-line bg-panel px-1 text-fg"
            value={detail}
            onChange={(e) => diagrams.setDetail(panel.id, e.target.value as Detail)}
          >
            <option value="all">{t("all")}</option>
            <option value="keys">{t("keys only")}</option>
            <option value="none">{t("none")}</option>
          </select>
        </label>
        <TextButton icon="refresh" label={t("Arrange")} disabled={tables.length === 0} onClick={() => { diagrams.rearrange(panel.id); fitted.current = false; }} />
        <TextButton icon="search" label={t("Fit")} disabled={tables.length === 0} onClick={fit} />
        <TextButton icon="copy" label={t("Export SVG…")} disabled={tables.length === 0} onClick={exportSvg} />
        <span className="ml-auto truncate text-faint tabular-nums">
          {panel.loading
            ? t("Reading…")
            : data
              ? t("{tables} of {all} tables · {references} references · {zoom}%", {
                  tables: tables.length,
                  all: data.all.length,
                  references: data.references.length,
                  zoom: Math.round(view.scale * 100),
                })
              : ""}
        </span>
      </div>
      {panel.error && (
        <p className="shrink-0 border-b border-line px-2 py-1.5 whitespace-pre-wrap text-danger select-text" role="alert">
          {panel.error}
        </p>
      )}
      <div className="relative min-h-0 flex-1">
        {data && tables.length === 0 && !panel.loading && (
          <p className="absolute inset-x-0 top-10 text-center text-muted">{t("No tables here. Add one by its name above.")}</p>
        )}
        <svg
          ref={svg}
          className="h-full w-full cursor-grab touch-none select-none active:cursor-grabbing"
          style={{ background: "var(--bg)" }}
          onPointerDown={(e) => onPointerDown(e, null)}
          onPointerMove={onPointerMove}
          onPointerUp={onPointerUp}
          onPointerCancel={onPointerUp}
        >
          <defs>
            <marker id={`${list}-one`} viewBox="0 0 10 10" refX="9" refY="5" markerWidth="9" markerHeight="9" orient="auto-start-reverse">
              <path d="M 4 0.5 L 4 9.5" fill="none" stroke="context-stroke" strokeWidth="1.4" />
            </marker>
            <marker id={`${list}-many`} viewBox="0 0 10 10" refX="9" refY="5" markerWidth="10" markerHeight="10" orient="auto-start-reverse">
              <path d="M 1 5 L 9 0.5 M 1 5 L 9 9.5 M 1 5 L 9 5" fill="none" stroke="context-stroke" strokeWidth="1.2" />
            </marker>
          </defs>
          <g data-world transform={`translate(${view.x} ${view.y}) scale(${view.scale})`}>
            {(data?.references ?? []).map((r) => {
              const from = boxes.get(r.from);
              const to = boxes.get(r.to);
              const fromTable = tables.find((table) => table.name === r.from);
              const toTable = tables.find((table) => table.name === r.to);
              if (!from || !to || !fromTable || !toTable) return null;
              const hot = selected !== null && (r.from === selected || r.to === selected);
              const line = path(r, from, to, rowOf(fromTable, detail, r.fromColumns[0]), rowOf(toTable, detail, r.toColumns[0]));
              return (
                <path
                  key={r.name}
                  d={line.d}
                  fill="none"
                  stroke={hot ? "var(--accent)" : "var(--muted)"}
                  strokeWidth={hot ? 1.8 : 1.1}
                  opacity={selected !== null && !hot ? 0.35 : 1}
                  markerStart={`url(#${list}-many)`}
                  markerEnd={`url(#${list}-one)`}
                >
                  <title>{`${r.name}: ${r.from} (${r.fromColumns.join(", ")}) → ${r.to} (${r.toColumns.join(", ")})`}</title>
                </path>
              );
            })}
            {tables.map((table) => {
              const box = boxes.get(table.name);
              if (!box) return null;
              const columns = shownColumns(table, detail);
              const isSelected = table.name === selected;
              const dim = selected !== null && !isSelected && !related.has(table.name);
              return (
                <g
                  key={table.name}
                  transform={`translate(${box.x} ${box.y})`}
                  opacity={dim ? 0.55 : 1}
                  className="cursor-move"
                  onPointerDown={(e) => onPointerDown(e, table.name)}
                  onPointerMove={onPointerMove}
                  onPointerUp={onPointerUp}
                  onPointerCancel={onPointerUp}
                  onDoubleClick={() => openObject(panel.registration, "table", table.name)}
                  onContextMenu={(e) => menu(e, table)}
                >
                  <rect width={box.w} height={box.h} rx={4} fill="var(--panel)" stroke={isSelected ? "var(--accent)" : "var(--border)"} strokeWidth={isSelected ? 1.8 : 1} />
                  <path d={`M 0 ${HEADER_H} L 0 4 Q 0 0 4 0 L ${box.w - 4} 0 Q ${box.w} 0 ${box.w} 4 L ${box.w} ${HEADER_H} Z`} fill={isSelected ? "var(--selected)" : "var(--hover)"} />
                  <text x={9} y={16} fontSize={12} fontWeight={600} fill="var(--text)">
                    {table.name}
                  </text>
                  {table.hidden.length > 0 && (
                    <text x={box.w - 8} y={16} fontSize={10.5} textAnchor="end" fill="var(--faint)">
                      +{table.hidden.length}
                      <title>{t("{n} related tables are not shown. Right-click to add them.", { n: table.hidden.length })}</title>
                    </text>
                  )}
                  {columns.map((column, i) => {
                    const y = HEADER_H + 2 + i * ROW_H + 12.5;
                    return (
                      <g key={column.name}>
                        <text x={9} y={y} fontSize={11.5} fontWeight={column.primaryKey ? 600 : 400} fill={column.primaryKey ? "var(--text)" : column.foreignKey ? "var(--accent)" : "var(--text)"}>
                          {column.name}
                          {column.required && !column.primaryKey ? " *" : ""}
                        </text>
                        <text x={box.w - 8} y={y} fontSize={10.5} textAnchor="end" fill="var(--muted)">
                          {column.primaryKey ? "PK " : ""}
                          {column.typeName}
                        </text>
                      </g>
                    );
                  })}
                </g>
              );
            })}
          </g>
        </svg>
      </div>
    </div>
  );
}

const COLORS = ["--bg", "--panel", "--border", "--text", "--muted", "--faint", "--accent", "--selected", "--hover"];

/** The diagram as a file of its own: the whole of it, not what the window
 *  shows, with the theme's colors written in and the app's font named */
function standalone(el: SVGSVGElement, boxes: Box[]): string {
  const copy = el.cloneNode(true) as SVGSVGElement;
  const left = Math.min(...boxes.map((b) => b.x)) - 30;
  const top = Math.min(...boxes.map((b) => b.y)) - 30;
  // Room on the right for the loops and curves that leave a box's side
  const width = Math.max(...boxes.map((b) => b.x + b.w)) - left + 90;
  const height = Math.max(...boxes.map((b) => b.y + b.h)) - top + 30;
  copy.querySelector("[data-world]")?.setAttribute("transform", `translate(${-left} ${-top})`);
  copy.removeAttribute("class");
  copy.removeAttribute("style");
  copy.setAttribute("xmlns", "http://www.w3.org/2000/svg");
  copy.setAttribute("width", String(Math.round(width)));
  copy.setAttribute("height", String(Math.round(height)));
  copy.setAttribute("viewBox", `0 0 ${Math.round(width)} ${Math.round(height)}`);
  copy.setAttribute("font-family", getComputedStyle(el).fontFamily);
  for (const node of copy.querySelectorAll("[opacity]")) node.removeAttribute("opacity");
  for (const node of copy.querySelectorAll("title")) node.remove();
  const style = getComputedStyle(document.documentElement);
  let text = new XMLSerializer().serializeToString(copy);
  for (const name of COLORS) text = text.replaceAll(`var(${name})`, style.getPropertyValue(name).trim());
  const background = `<rect width="100%" height="100%" fill="${style.getPropertyValue("--bg").trim()}"/>`;
  return `<?xml version="1.0" encoding="UTF-8"?>\n${text.replace(/(<svg[^>]*>)/, `$1${background}`)}\n`;
}
