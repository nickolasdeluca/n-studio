// What was done to a grid's rows and is not in the table yet. A fetched row
// is known by its key's values, not by where the grid shows it: sorting
// moves rows, and a fetch brings more.
import type { Cell, RowChanges } from "./api";

export interface RowEdits {
  /** Fetched rows given new values, by `rowId`; `set` is by column name */
  changed: Record<string, { key: Cell[]; set: Record<string, Cell> }>;
  /** Fetched rows to delete, by `rowId` */
  deleted: Record<string, Cell[]>;
  /** New rows, shown after the fetched ones: the columns given a value */
  inserted: Record<string, Cell>[];
}

export const noEdits: RowEdits = { changed: {}, deleted: {}, inserted: [] };

export const rowId = (key: Cell[]) => JSON.stringify(key);

export const editCount = (e: RowEdits) =>
  Object.keys(e.changed).length + Object.keys(e.deleted).length + e.inserted.length;

/** A fetched row's cell given a value; back at `original`, it is no change */
export function setCell(e: RowEdits, key: Cell[], column: string, value: Cell, original: Cell): RowEdits {
  const id = rowId(key);
  const set = { ...e.changed[id]?.set };
  if (value === original) delete set[column];
  else set[column] = value;
  const changed = { ...e.changed };
  if (Object.keys(set).length === 0) delete changed[id];
  else changed[id] = { key, set };
  return { ...e, changed };
}

/** A new row's cell given a value */
export function setInserted(e: RowEdits, index: number, column: string, value: Cell): RowEdits {
  return { ...e, inserted: e.inserted.map((row, i) => (i === index ? { ...row, [column]: value } : row)) };
}

/** Marks a fetched row to be deleted, or takes the mark back */
export function toggleDeleted(e: RowEdits, key: Cell[]): RowEdits {
  const id = rowId(key);
  const deleted = { ...e.deleted };
  if (id in deleted) delete deleted[id];
  else deleted[id] = key;
  return { ...e, deleted };
}

export const addInserted = (e: RowEdits): RowEdits => ({ ...e, inserted: [...e.inserted, {}] });

export const removeInserted = (e: RowEdits, index: number): RowEdits => ({
  ...e,
  inserted: e.inserted.filter((_, i) => i !== index),
});

/** The edits as the backend takes them. A row both changed and deleted is
 *  only deleted. */
export function toChanges(e: RowEdits): RowChanges {
  const assigns = (set: Record<string, Cell>) => Object.entries(set).map(([column, value]) => ({ column, value }));
  return {
    updates: Object.entries(e.changed)
      .filter(([id]) => !(id in e.deleted))
      .map(([, row]) => ({ key: row.key, set: assigns(row.set) })),
    deletes: Object.values(e.deleted),
    inserts: e.inserted.map(assigns),
  };
}
