// Run by `npm test`
import assert from "node:assert/strict";
import { test } from "node:test";
import { aggregatesFor, valueOf } from "./summary.ts";

test("numbers add up; text and dates count and compare; blobs only count", () => {
  assert.deepEqual(aggregatesFor("decimal"), ["sum", "avg", "count", "distinct", "min", "max"]);
  assert.deepEqual(aggregatesFor("text"), ["count", "distinct", "min", "max"]);
  assert.deepEqual(aggregatesFor("timestamp"), ["count", "distinct", "min", "max"]);
  assert.deepEqual(aggregatesFor("text_blob"), ["count"]);
});

test("a summary's values, and the ones it has none of", () => {
  const s = { count: 3, distinct: 2, min: "7", max: "23", sum: "37", avg: "12.3333", overflow: false };
  assert.equal(valueOf(s, "sum"), "37");
  assert.equal(valueOf(s, "count"), "3");
  assert.equal(valueOf(s, "distinct"), "2");
  const empty = { count: 0, distinct: 0, min: null, max: null, sum: "0", avg: null, overflow: false };
  assert.equal(valueOf(empty, "avg"), null);
  assert.equal(valueOf(empty, "count"), "0");
});
