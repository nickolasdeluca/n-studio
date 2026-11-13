// A query put together from parts: the tables it reads, how they join,
// and what each column is there for. The parts are kept here; the backend
// writes the statement, so it quotes names as every other one does.
import { create } from "zustand";
import { api, errorText, type DiagramReference, type DiagramTable, type QueryColumn, type QueryJoin, type QuerySpec } from "./api";
import { openAdminPanel } from "./dock";
import { t } from "./i18n";
import { useStore } from "./store";

export interface BuilderPanel {
  id: string;
  registration: string;
  /** Every table and view of the database */
  all: string[];
  /** The ones the query reads, by name */
  defs: Record<string, DiagramTable>;
  references: DiagramReference[];
  spec: QuerySpec;
  sql: string;
  loading: boolean;
  error?: string;
}

const empty: QuerySpec = { tables: [], joins: [], columns: [], distinct: false, first: null };

/** The joins a table brings when it first enters a query: one for each
 *  foreign key between it and a table already there */
export function joinsFor(spec: QuerySpec, references: DiagramReference[], name: string, alias: string): QueryJoin[] {
  const joins: QueryJoin[] = [];
  const aliasOf = (table: string) => spec.tables.find((x) => x.name === table)?.alias;
  for (const r of references) {
    const on = r.fromColumns.map((c, i): [string, string] => [c, r.toColumns[i]]);
    if (r.from === name) {
      const to = aliasOf(r.to);
      if (to) joins.push({ kind: "inner", left: alias, right: to, on });
    } else if (r.to === name) {
      const from = aliasOf(r.from);
      if (from) joins.push({ kind: "inner", left: from, right: alias, on });
    }
  }
  return joins;
}

/** A name for a table's next appearance: its own, then with a number */
export function aliasFor(spec: QuerySpec, name: string): string {
  const taken = new Set(spec.tables.map((x) => x.alias));
  if (!taken.has(name)) return name;
  for (let n = 2; ; n++) if (!taken.has(`${name}_${n}`)) return `${name}_${n}`;
}

interface Builders {
  panels: Record<string, BuilderPanel>;
  /** Changes a panel's parts and asks for the statement they make */
  change(id: string, to: (spec: QuerySpec) => QuerySpec): void;
  addTable(id: string, name: string): Promise<void>;
  removeTable(id: string, alias: string): void;
  renameTable(id: string, alias: string, to: string): boolean;
  toggleColumn(id: string, alias: string, column: string): void;
  patchColumn(id: string, index: number, p: Partial<QueryColumn>): void;
  moveColumn(id: string, index: number, by: number): void;
  removeColumn(id: string, index: number): void;
  addJoin(id: string): void;
  patchJoin(id: string, index: number, p: Partial<QueryJoin>): void;
  removeJoin(id: string, index: number): void;
  close(id: string): void;
}

/** Which request for a panel's statement is the latest: answers to older
 *  ones are dropped */
const asked = new Map<string, number>();

export const useBuilders = create<Builders>((set, get) => {
  const patch = (id: string, p: Partial<BuilderPanel>) =>
    set((s) => (s.panels[id] ? { panels: { ...s.panels, [id]: { ...s.panels[id], ...p } } } : s));

  const write = async (id: string) => {
    const panel = get().panels[id];
    if (!panel) return;
    const turn = (asked.get(id) ?? 0) + 1;
    asked.set(id, turn);
    try {
      const sql = panel.spec.tables.length > 0 ? await api.queryBuild(panel.registration, panel.spec) : "";
      if (asked.get(id) === turn) patch(id, { sql, error: undefined });
    } catch (e) {
      if (asked.get(id) === turn) patch(id, { error: errorText(e) });
    }
  };

  const change: Builders["change"] = (id, to) => {
    const panel = get().panels[id];
    if (!panel) return;
    patch(id, { spec: to(panel.spec) });
    void write(id);
  };

  return {
    panels: {},
    change,
    async addTable(id, name) {
      const panel = get().panels[id];
      if (!panel) return;
      patch(id, { loading: true, error: undefined });
      try {
        const names = [...new Set([...panel.spec.tables.map((x) => x.name), name])];
        const data = await api.querySources(panel.registration, names);
        const defs = Object.fromEntries(data.tables.map((table) => [table.name, table]));
        patch(id, { loading: false, all: data.all, defs, references: data.references });
        if (!defs[name]) {
          if (name) patch(id, { error: t("There is no table or view named {name}.", { name }) });
          return;
        }
        change(id, (spec) => {
          const alias = aliasFor(spec, name);
          // There a second time, it is for a join of the user's own: the
          // keys that joined the first would only tie the two together
          const joins = alias === name ? joinsFor(spec, data.references, name, alias) : [];
          return { ...spec, tables: [...spec.tables, { name, alias }], joins: [...spec.joins, ...joins] };
        });
      } catch (e) {
        patch(id, { loading: false, error: errorText(e) });
      }
    },
    removeTable: (id, alias) =>
      change(id, (spec) => ({
        ...spec,
        tables: spec.tables.filter((x) => x.alias !== alias),
        joins: spec.joins.filter((j) => j.left !== alias && j.right !== alias),
        columns: spec.columns.filter((c) => c.table !== alias),
      })),
    renameTable(id, alias, to) {
      const panel = get().panels[id];
      const next = to.trim();
      if (!panel || !next || next === alias) return next === alias;
      if (panel.spec.tables.some((x) => x.alias === next)) return false;
      const renamed = (x: string) => (x === alias ? next : x);
      change(id, (spec) => ({
        ...spec,
        tables: spec.tables.map((x) => (x.alias === alias ? { ...x, alias: next } : x)),
        joins: spec.joins.map((j) => ({ ...j, left: renamed(j.left), right: renamed(j.right) })),
        columns: spec.columns.map((c) => ({ ...c, table: renamed(c.table) })),
      }));
      return true;
    },
    toggleColumn: (id, alias, column) =>
      change(id, (spec) => {
        const is = (c: QueryColumn) => c.table === alias && c.column === column;
        return spec.columns.some(is)
          ? { ...spec, columns: spec.columns.filter((c) => !is(c)) }
          : { ...spec, columns: [...spec.columns, { table: alias, column, alias: null, aggregate: null, sort: null, filter: null, output: true }] };
      }),
    patchColumn: (id, index, p) =>
      change(id, (spec) => ({ ...spec, columns: spec.columns.map((c, i) => (i === index ? { ...c, ...p } : c)) })),
    moveColumn: (id, index, by) =>
      change(id, (spec) => {
        const to = index + by;
        if (to < 0 || to >= spec.columns.length) return spec;
        const columns = [...spec.columns];
        [columns[index], columns[to]] = [columns[to], columns[index]];
        return { ...spec, columns };
      }),
    removeColumn: (id, index) => change(id, (spec) => ({ ...spec, columns: spec.columns.filter((_, i) => i !== index) })),
    addJoin: (id) =>
      change(id, (spec) => {
        if (spec.tables.length < 2) return spec;
        const [left, right] = [spec.tables[0], spec.tables[spec.tables.length - 1]];
        const defs = get().panels[id]?.defs ?? {};
        const first = (name: string) => defs[name]?.columns[0]?.name ?? "";
        return { ...spec, joins: [...spec.joins, { kind: "inner", left: left.alias, right: right.alias, on: [[first(left.name), first(right.name)]] }] };
      }),
    patchJoin: (id, index, p) => change(id, (spec) => ({ ...spec, joins: spec.joins.map((j, i) => (i === index ? { ...j, ...p } : j)) })),
    removeJoin: (id, index) => change(id, (spec) => ({ ...spec, joins: spec.joins.filter((_, i) => i !== index) })),
    close(id) {
      asked.delete(id);
      set((s) => {
        const panels = { ...s.panels };
        delete panels[id];
        return { panels };
      });
    },
  };
});

let count = 0;

/** Opens a query builder on a connected registration, with `tables`
 *  already in it */
export async function openBuilder(registration: string, tables: string[] = []) {
  const s = useStore.getState();
  if (!s.servers[registration]) return;
  const name = s.registrations.find((r) => r.id === registration)?.name ?? "";
  const id = `adm-qry-${++count}`;
  useBuilders.setState((state) => ({
    panels: { ...state.panels, [id]: { id, registration, all: [], defs: {}, references: [], spec: empty, sql: "", loading: false } },
  }));
  openAdminPanel(id, "builder", `${t("Query Builder")} ${count}${name ? ` · ${name}` : ""}`);
  // One after the other: each finds the ones before it to join with
  if (tables.length === 0) await useBuilders.getState().addTable(id, "");
  for (const table of tables) await useBuilders.getState().addTable(id, table);
}
