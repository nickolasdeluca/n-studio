// The grid's summary row: what each column adds up to, chosen per column.
// Counting, the different values, the least and the greatest work on any
// column but a blob's, whose cells are markers; a sum and an average only on
// numbers.
import type { ColumnKind, ColumnSummary } from "./api";

export type Aggregate = "sum" | "avg" | "count" | "distinct" | "min" | "max";

export const AGGREGATES: { value: Aggregate; label: string; short: string }[] = [
  { value: "sum", label: "Sum", short: "SUM" },
  { value: "avg", label: "Average", short: "AVG" },
  { value: "count", label: "Count", short: "COUNT" },
  { value: "distinct", label: "Count Distinct", short: "DISTINCT" },
  { value: "min", label: "Minimum", short: "MIN" },
  { value: "max", label: "Maximum", short: "MAX" },
];

const numeric = (k: ColumnKind) => k === "integer" || k === "decimal" || k === "float";
const blob = (k: ColumnKind) => k === "blob" || k === "text_blob";

/** What a column of this kind can be summed up by */
export function aggregatesFor(kind: ColumnKind): Aggregate[] {
  if (blob(kind)) return ["count"];
  if (numeric(kind)) return ["sum", "avg", "count", "distinct", "min", "max"];
  return ["count", "distinct", "min", "max"];
}

/** The value an aggregate reads from a summary: null when the column has
 *  none (no values, or a sum past 38 digits) */
export function valueOf(summary: ColumnSummary, aggregate: Aggregate): string | null {
  switch (aggregate) {
    case "count":
      return String(summary.count);
    case "distinct":
      return summary.distinct === null ? null : String(summary.distinct);
    default:
      return summary[aggregate];
  }
}

export const shortOf = (aggregate: Aggregate) => AGGREGATES.find((a) => a.value === aggregate)?.short ?? aggregate;
