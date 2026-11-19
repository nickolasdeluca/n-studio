// Run by `npm test`: Node's own runner, reading the TypeScript as it is.
import assert from "node:assert/strict";
import { test } from "node:test";
import { lineDiff } from "./lineDiff.ts";

const kinds = (d) => d.map((l) => `${l.kind[0]} ${l.text}`);

test("the lines both texts have are kept, the others removed or added", () => {
  const d = lineDiff("create table t (\n  a int,\n  b int\n)", "create table t (\n  a int,\n  c varchar(10),\n  b int\n)");
  assert.deepEqual(kinds(d), ["s create table t (", "s   a int,", "a   c varchar(10),", "s   b int", "s )"]);
});

test("a changed line is removed and added, and trailing spaces do not count", () => {
  const d = lineDiff("x \r\ny\nz", "x\nY\nz");
  assert.deepEqual(kinds(d), ["s x", "r y", "a Y", "s z"]);
});

test("one text empty is all added or all removed", () => {
  assert.deepEqual(kinds(lineDiff("", "a")), ["r ", "a a"]);
  assert.deepEqual(kinds(lineDiff("a\nb", "a\nb")), ["s a", "s b"]);
});
