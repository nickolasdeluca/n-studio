// A diagram of tables and the foreign keys between them: which tables it
// shows and where each sits. The backend says what the tables are; where
// they go is decided here, once, and from then on by whoever drags them.
import { create } from "zustand";
import { api, errorText, type Diagram, type DiagramTable } from "./api";
import { openAdminPanel } from "./dock";
import { t } from "./i18n";
import { useStore } from "./store";

export type Detail = "all" | "keys" | "none";

export interface Point {
  x: number;
  y: number;
}

export interface DiagramPanel {
  id: string;
  registration: string;
  data?: Diagram;
  loading: boolean;
  error?: string;
  /** Where each table's top left corner is, by table */
  positions: Record<string, Point>;
  /** Which columns the tables show */
  detail: Detail;
}

export const HEADER_H = 24;
export const ROW_H = 17;
const CHAR_W = 6.7;

export const shownColumns = (table: DiagramTable, detail: Detail) =>
  detail === "all" ? table.columns : detail === "keys" ? table.columns.filter((c) => c.primaryKey || c.foreignKey) : [];

/** A table's box: as wide as its longest line, as tall as its columns */
export function sizeOf(table: DiagramTable, detail: Detail): { w: number; h: number } {
  const columns = shownColumns(table, detail);
  const longest = Math.max(table.name.length + 2, ...columns.map((c) => c.name.length + c.typeName.length + 5));
  return {
    w: Math.round(Math.min(360, Math.max(130, longest * CHAR_W + 20))),
    h: HEADER_H + columns.length * ROW_H + (columns.length > 0 ? 4 : 0),
  };
}

const GAP_X = 90;
const GAP_Y = 28;

/** Where the tables go when nobody has put them anywhere: each group of
 *  related tables in columns by how far it is from the group's most
 *  referenced table, the groups side by side, tables related to nothing in
 *  rows after them. */
export function arrange(data: Diagram, detail: Detail): Record<string, Point> {
  const sizes = new Map(data.tables.map((table) => [table.name, sizeOf(table, detail)]));
  const near = new Map<string, Set<string>>(data.tables.map((table) => [table.name, new Set()]));
  for (const r of data.references) {
    if (r.from === r.to) continue;
    near.get(r.from)?.add(r.to);
    near.get(r.to)?.add(r.from);
  }

  // The groups, and each one's tables by distance from its hub
  const placed = new Set<string>();
  const groups: string[][][] = [];
  const alone: string[] = [];
  const byDegree = [...near.keys()].sort((a, b) => near.get(b)!.size - near.get(a)!.size || a.localeCompare(b));
  for (const hub of byDegree) {
    if (placed.has(hub)) continue;
    if (near.get(hub)!.size === 0) {
      placed.add(hub);
      alone.push(hub);
      continue;
    }
    const layers: string[][] = [];
    let layer = [hub];
    placed.add(hub);
    while (layer.length > 0) {
      layers.push(layer);
      const next: string[] = [];
      for (const table of layer) {
        for (const other of [...near.get(table)!].sort()) {
          if (!placed.has(other)) {
            placed.add(other);
            next.push(other);
          }
        }
      }
      layer = next;
    }
    groups.push(layers);
  }

  const positions: Record<string, Point> = {};
  // Groups fill rows about as wide as the whole is tall
  const area = [...sizes.values()].reduce((sum, s) => sum + (s.w + GAP_X) * (s.h + GAP_Y), 0);
  const rowWidth = Math.max(1200, Math.sqrt(area) * 1.5);
  let left = 0;
  let top = 0;
  let rowHeight = 0;
  const shelf = (width: number, height: number) => {
    if (left > 0 && left + width > rowWidth) {
      left = 0;
      top += rowHeight + GAP_Y * 3;
      rowHeight = 0;
    }
    const at = { x: left, y: top };
    left += width + GAP_X * 1.5;
    rowHeight = Math.max(rowHeight, height);
    return at;
  };

  for (const layers of groups) {
    // Tables of a column go in the order of what they hang from, so lines
    // cross less
    const order = new Map<string, number>();
    const columns = layers.map((layer, i) => {
      const sorted =
        i === 0
          ? layer
          : [...layer].sort((a, b) => {
              const rank = (table: string) => {
                const above = [...near.get(table)!].map((n) => order.get(n)).filter((n): n is number => n !== undefined);
                return above.length > 0 ? above.reduce((x, y) => x + y, 0) / above.length : 0;
              };
              return rank(a) - rank(b) || a.localeCompare(b);
            });
      sorted.forEach((table, row) => order.set(table, row));
      return sorted;
    });
    const widths = columns.map((column) => Math.max(...column.map((table) => sizes.get(table)!.w)));
    const heights = columns.map((column) => column.reduce((sum, table) => sum + sizes.get(table)!.h + GAP_Y, -GAP_Y));
    const height = Math.max(...heights);
    const origin = shelf(widths.reduce((sum, w) => sum + w + GAP_X, -GAP_X), height);
    let x = origin.x;
    columns.forEach((column, i) => {
      // Each column centered on the tallest
      let y = origin.y + (height - heights[i]) / 2;
      for (const table of column) {
        positions[table] = { x: Math.round(x), y: Math.round(y) };
        y += sizes.get(table)!.h + GAP_Y;
      }
      x += widths[i] + GAP_X;
    });
  }

  if (alone.length > 0) {
    if (left > 0) {
      left = 0;
      top += rowHeight + GAP_Y * 3;
      rowHeight = 0;
    }
    for (const table of alone.sort()) {
      const size = sizes.get(table)!;
      if (left > 0 && left + size.w > rowWidth) {
        left = 0;
        top += rowHeight + GAP_Y;
        rowHeight = 0;
      }
      positions[table] = { x: Math.round(left), y: Math.round(top) };
      left += size.w + 40;
      rowHeight = Math.max(rowHeight, size.h);
    }
  }
  return positions;
}

/** Positions for the tables that have none: beside a table they are related
 *  to when there is one, under everything otherwise */
function placeNew(data: Diagram, detail: Detail, known: Record<string, Point>): Record<string, Point> {
  const positions: Record<string, Point> = {};
  for (const table of data.tables) if (known[table.name]) positions[table.name] = known[table.name];
  const fresh = data.tables.filter((table) => !positions[table.name]);
  if (fresh.length === 0) return positions;
  if (fresh.length === data.tables.length) return arrange(data, detail);

  const sizes = new Map(data.tables.map((table) => [table.name, sizeOf(table, detail)]));
  const boxes = () => Object.entries(positions).map(([name, p]) => ({ ...p, ...sizes.get(name)! }));
  const free = (x: number, y: number, w: number, h: number) =>
    boxes().every((b) => x + w + 20 <= b.x || b.x + b.w + 20 <= x || y + h + 12 <= b.y || b.y + b.h + 12 <= y);
  const bottom = () => Math.max(...boxes().map((b) => b.y + b.h));
  for (const table of fresh) {
    const size = sizes.get(table.name)!;
    const anchor = data.references
      .filter((r) => r.from === table.name || r.to === table.name)
      .map((r) => (r.from === table.name ? r.to : r.from))
      .find((name) => positions[name]);
    let at: Point | null = null;
    if (anchor) {
      const a = { ...positions[anchor], ...sizes.get(anchor)! };
      // To the right of what it is related to, then to the left, going down
      for (let step = 0; step < 40 && !at; step++) {
        const y = a.y + step * (ROW_H * 4);
        if (free(a.x + a.w + GAP_X, y, size.w, size.h)) at = { x: a.x + a.w + GAP_X, y };
        else if (free(a.x - GAP_X - size.w, y, size.w, size.h)) at = { x: a.x - GAP_X - size.w, y };
      }
    }
    positions[table.name] = at ?? { x: Math.min(...boxes().map((b) => b.x)), y: bottom() + GAP_Y * 2 };
  }
  return positions;
}

interface DiagramState {
  panels: Record<string, DiagramPanel>;
  /** Asks for the diagram of `tables` and what is within `depth` references */
  load(id: string, tables: string[], depth: number): Promise<void>;
  add(id: string, tables: string[]): Promise<void>;
  remove(id: string, table: string): Promise<void>;
  move(id: string, table: string, to: Point): void;
  setDetail(id: string, detail: Detail): void;
  rearrange(id: string): void;
  close(id: string): void;
}

export const useDiagrams = create<DiagramState>((set, get) => {
  const patch = (id: string, p: Partial<DiagramPanel>) =>
    set((s) => (s.panels[id] ? { panels: { ...s.panels, [id]: { ...s.panels[id], ...p } } } : s));
  const shown = (id: string) => get().panels[id]?.data?.tables.map((table) => table.name) ?? [];
  return {
    panels: {},
    async load(id, tables, depth) {
      const panel = get().panels[id];
      if (!panel || panel.loading) return;
      patch(id, { loading: true });
      try {
        const data = await api.diagram(panel.registration, tables, depth);
        const now = get().panels[id];
        if (now) patch(id, { data, loading: false, error: undefined, positions: placeNew(data, now.detail, now.positions) });
      } catch (e) {
        patch(id, { loading: false, error: errorText(e) });
      }
    },
    add: (id, tables) => get().load(id, [...shown(id), ...tables], 0),
    async remove(id, table) {
      const rest = shown(id).filter((name) => name !== table);
      // No table named is every table: the last one leaves an empty diagram
      if (rest.length === 0) patch(id, { data: { tables: [], references: [], all: get().panels[id]?.data?.all ?? [] } });
      else await get().load(id, rest, 0);
    },
    move(id, table, to) {
      const panel = get().panels[id];
      if (panel) patch(id, { positions: { ...panel.positions, [table]: to } });
    },
    setDetail: (id, detail) => patch(id, { detail }),
    rearrange(id) {
      const panel = get().panels[id];
      if (panel?.data) patch(id, { positions: arrange(panel.data, panel.detail) });
    },
    close(id) {
      set((s) => {
        const panels = { ...s.panels };
        delete panels[id];
        return { panels };
      });
    },
  };
});

let count = 0;

/** Opens a diagram on a connected registration: of `tables` and what they
 *  are related to, or of the whole database */
export function openDiagram(registration: string, tables: string[] = []) {
  const s = useStore.getState();
  if (!s.servers[registration]) return;
  const name = s.registrations.find((r) => r.id === registration)?.name ?? "";
  const id = `adm-dgm-${++count}`;
  useDiagrams.setState((state) => ({
    panels: { ...state.panels, [id]: { id, registration, loading: false, positions: {}, detail: "all" } },
  }));
  const title = tables.length === 1 ? tables[0] : name;
  openAdminPanel(id, "diagram", `${t("Diagram")}${title ? ` · ${title}` : ""}`);
  void useDiagrams.getState().load(id, tables, tables.length > 0 ? 1 : 0);
}
