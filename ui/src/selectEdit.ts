// A select's rows as rows of the table they come from: whether they can be
// changed in the grid, and which of the select's columns is which of the
// table's. The server says where each column comes from; the statement's
// text says how many times the table is read.
//
// No imports but types: `npm test` runs this file as it is.
import type { ColumnInfo, RowEditing } from "./api";

/** The kinds of column whose values the grid shows as they can be typed */
export const typed = new Set(["integer", "decimal", "float", "text", "date", "time", "timestamp", "boolean"]);

/** The one table a select's columns come from, if its rows could be
 *  changed through them: every column that is a table's is that table's,
 *  and the statement reads it once (`read`: the relations it names as
 *  sources). Read twice, as a join of a table with
 *  itself is, two columns of one row could be two rows'. */
export function tableOf(columns: ColumnInfo[], read: string[]): string | null {
  const relations = new Set(columns.filter((c) => c.relation && c.field).map((c) => c.relation));
  if (relations.size !== 1) return null;
  const [table] = relations;
  return read.filter((relation) => relation === table).length > 1 ? null : table;
}

/** A select's rows as rows of a table that can be changed */
export interface SelectEditing {
  table: string;
  /** The key's columns, as places among the select's */
  key: number[];
  /** By the select's columns: whether a value can be typed into it */
  editable: boolean[];
  /** By the select's columns: the table's column, whatever it is headed with */
  names: string[];
}

/** How a select's rows can be changed, given how its table's can: `null`
 *  when the select did not bring the whole key. A column brought twice is
 *  typed into where it comes first. */
export function selectEditing(columns: ColumnInfo[], table: string, editing: RowEditing): SelectEditing | null {
  const own = (c: ColumnInfo) => c.relation === table && c.field !== "";
  const place = (name: string) => columns.findIndex((c) => own(c) && c.field === name);
  const key = editing.key.map(place);
  if (key.some((k) => k < 0 || !typed.has(columns[k].kind))) return null;
  return {
    table,
    key,
    editable: columns.map((c, i) => own(c) && place(c.field) === i && editing.editable.includes(c.field) && typed.has(c.kind)),
    names: columns.map((c, i) => (own(c) ? c.field : `\u0000${i}`)),
  };
}
