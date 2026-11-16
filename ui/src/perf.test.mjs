import assert from "node:assert/strict";
import { test } from "node:test";
import { summarize } from "./perf.ts";

const table = (name, counts = {}) => ({
  table: name, indexedReads: 0, nonIndexedReads: 0, inserts: 0, updates: 0, deletes: 0, backouts: 0, purges: 0, expunges: 0, ...counts,
});
const perf = (tables, counts = {}) => ({ reads: 0, writes: 0, fetches: 1000, marks: 0, currentMemory: 0, maxMemory: 0, buffers: 0, pageSize: 0, tables, ...counts });
const kinds = (s) => s.findings.map((f) => f.kind);

test("a select read through indexes, prepared for the first time", () => {
  const s = summarize({
    prepareMs: 52, executeMs: 0.5, fetchMs: 7.1, rows: 100, named: ["PEDIDO", "PEDIDO_ITEM"],
    performance: perf([table("PEDIDO", { indexedReads: 100 }), table("PEDIDO_ITEM", { indexedReads: 7789 })]),
  });
  assert.equal(Math.round(s.totalMs * 10), 596);
  assert.equal(s.read, 7889);
  assert.deepEqual(s.per, { rows: 100, what: "returned" });
  assert.equal(s.cached, 1);
  // The table read most comes first
  assert.equal(s.tables[0].table, "PEDIDO_ITEM");
  assert.deepEqual(kinds(s), ["Prepare", "Join"]);
  assert.equal(s.findings[1].vars.times, "78");
});

test("an update that reads a table whole and fires triggers", () => {
  const s = summarize({
    prepareMs: 1.4, executeMs: 2184, fetchMs: null, rows: null, named: ["PEDIDO_ITEM"],
    performance: perf(
      [table("ESTOQUE", { indexedReads: 212, updates: 212 }), table("PEDIDO_ITEM", { nonIndexedReads: 1284300, updates: 212, purges: 96 })],
      { reads: 9412, fetches: 2612480 },
    ),
  });
  assert.deepEqual(s.per, { rows: 424, what: "changed" });
  assert.equal(s.changed.tables, 2);
  assert.deepEqual(kinds(s), ["Scan", "Disk", "Indirect", "Cleaning"]);
  assert.equal(s.findings[2].vars.tables, "ESTOQUE");
  assert.ok(s.cached > 0.99 && s.cached < 1);
});

test("a small table read whole is nothing to point at", () => {
  const s = summarize({
    prepareMs: 1, executeMs: 1, fetchMs: 1, rows: 3, named: ["TIPO"],
    performance: perf([table("TIPO", { nonIndexedReads: 12 })]),
  });
  assert.deepEqual(kinds(s), ["Fine"]);
});

test("without counters there are only the times", () => {
  const s = summarize({ prepareMs: 2, executeMs: 3, fetchMs: null, rows: null, named: [], performance: null });
  assert.equal(s.totalMs, 5);
  assert.deepEqual(s.findings, []);
  assert.equal(s.per, null);
});
