// A statement's parameters: the editor asks their values before it runs,
// and remembers them per tab for the next run. A `:name` used twice is one
// value; each `?` is its own.
import type { ColumnInfo } from "./api";
import { useStore } from "./store";

export interface ParamField {
  /** The name, or `?1`, `?2`… by position for a `?` */
  key: string;
  /** As the server declares it: `INTEGER`, `VARCHAR(60)`, … */
  type: ColumnInfo;
  /** The statement's positions it fills */
  positions: number[];
}

export interface ParamValue {
  value: string;
  isNull: boolean;
}

export interface ParamRequest {
  tab: string;
  fields: ParamField[];
  initial: Record<string, ParamValue>;
  /** The values by key, or `null` when the user cancelled */
  done(values: Record<string, ParamValue> | null): void;
}

const remembered = new Map<string, Record<string, ParamValue>>();

/** The fields to ask for, from each position's name and declared type */
export function paramFields(names: (string | null)[], types: ColumnInfo[]): ParamField[] {
  const fields: ParamField[] = [];
  const byName = new Map<string, ParamField>();
  names.forEach((name, i) => {
    const known = name !== null ? byName.get(name) : undefined;
    if (known) {
      known.positions.push(i);
      return;
    }
    const field = { key: name ?? `?${i + 1}`, type: types[i], positions: [i] };
    fields.push(field);
    if (name !== null) byName.set(name, field);
  });
  return fields;
}

/** Asks the parameters' values; `null` when the user cancelled. A value is
 *  sent as text, which the server converts; `null` is SQL NULL. */
export function askParams(tab: string, fields: ParamField[]): Promise<(string | null)[] | null> {
  return new Promise((resolve) => {
    const done = (values: Record<string, ParamValue> | null) => {
      if (!values) return resolve(null);
      remembered.set(tab, { ...remembered.get(tab), ...values });
      const out: (string | null)[] = [];
      for (const f of fields) for (const p of f.positions) out[p] = values[f.key].isNull ? null : values[f.key].value;
      resolve(out);
    };
    const initial = remembered.get(tab) ?? {};
    useStore.getState().openDialog({ type: "params", request: { tab, fields, initial, done } });
  });
}

export function forgetParams(tab: string): void {
  remembered.delete(tab);
}
