// The order a grid's rows are shown in once a header is clicked: by that
// column, up, then down, then back to the order they came in.
//
// No imports: `npm test` runs this file as it is.

export interface GridSort {
  column: number;
  descending: boolean;
}

/** What a click on the header of `column` makes of the sort there was */
export function nextSort(sort: GridSort | null, column: number): GridSort | null {
  if (sort?.column !== column) return { column, descending: false };
  return sort.descending ? null : { column, descending: true };
}

const text = new Intl.Collator(undefined, { numeric: true, sensitivity: "base" });

/**
 * The places of `rows` in the order `sort` shows them. A number column
 * compares as numbers, anything else as text, with the digits in it read as
 * numbers (`F2` before `F10`). Empty cells go last either way, and rows that
 * compare equal keep the order they came in.
 */
export function sortedOrder(
  rows: { cells: (string | null)[] }[],
  kind: string | undefined,
  sort: GridSort | null,
): number[] {
  const order = rows.map((_, i) => i);
  if (!sort) return order;
  const cell = (i: number) => {
    const value = rows[i].cells[sort.column];
    return value === null || value === undefined || value === "" ? null : value;
  };
  const compare = (a: string, b: string) => {
    if (kind === "number") {
      const [x, y] = [Number(a.replace(/[\s,]/g, "")), Number(b.replace(/[\s,]/g, ""))];
      if (!Number.isNaN(x) && !Number.isNaN(y) && x !== y) return x - y;
    }
    return text.compare(a, b);
  };
  const sign = sort.descending ? -1 : 1;
  return order.sort((i, j) => {
    const [a, b] = [cell(i), cell(j)];
    if (a === null || b === null) return a === b ? i - j : a === null ? 1 : -1;
    return sign * compare(a, b) || i - j;
  });
}
