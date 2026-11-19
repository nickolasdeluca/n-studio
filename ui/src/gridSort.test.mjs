// Run by `npm test`: Node's own runner, reading the TypeScript as it is.
import assert from "node:assert/strict";
import { test } from "node:test";
import { nextSort, sortedOrder } from "./gridSort.ts";

const rows = (...values) => values.map((v) => ({ cells: [v] }));

test("a header's clicks sort up, then down, then not at all", () => {
  const up = nextSort(null, 2);
  assert.deepEqual(up, { column: 2, descending: false });
  const down = nextSort(up, 2);
  assert.deepEqual(down, { column: 2, descending: true });
  assert.equal(nextSort(down, 2), null);
  assert.deepEqual(nextSort(down, 0), { column: 0, descending: false });
});

test("without a sort the rows keep the order they came in", () => {
  assert.deepEqual(sortedOrder(rows("B", "A"), "text", null), [0, 1]);
});

test("text sorts without regard to case, reading its digits as numbers", () => {
  assert.deepEqual(sortedOrder(rows("F10", "f2", "F1"), "text", { column: 0, descending: false }), [2, 1, 0]);
  assert.deepEqual(sortedOrder(rows("F10", "f2", "F1"), "text", { column: 0, descending: true }), [0, 1, 2]);
});

test("a number column sorts as numbers", () => {
  assert.deepEqual(sortedOrder(rows("100", "-3", "20.5"), "number", { column: 0, descending: false }), [1, 2, 0]);
});

test("empty cells go last both ways, and equal rows keep their order", () => {
  const some = rows(null, "✓", "", "✓");
  assert.deepEqual(sortedOrder(some, "flag", { column: 0, descending: false }), [1, 3, 0, 2]);
  assert.deepEqual(sortedOrder(some, "flag", { column: 0, descending: true }), [1, 3, 0, 2]);
});
