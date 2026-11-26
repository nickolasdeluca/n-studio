// Run by `npm test`: Node's own runner, reading the TypeScript as it is.
import assert from "node:assert/strict";
import { test } from "node:test";
import { addFiles, byName, clearDone, emptyQueue, fileName, move, nextItem, patchItem, remove, reset, sortByName } from "./scriptQueue.ts";

const names = (q) => q.items.map((i) => fileName(i.path));

test("files are ordered by name as migrations are, numbers as numbers", () => {
  const paths = ["/m/V10__index.sql", "/m/V2__table.sql", "/m/V1__init.sql", "/m/V1.1__fix.sql"];
  assert.deepEqual(paths.sort(byName).map(fileName), ["V1__init.sql", "V1.1__fix.sql", "V2__table.sql", "V10__index.sql"]);
  const stamps = ["C:\\m\\20240201_b.sql", "C:\\m\\20240115_a.sql", "C:\\m\\002_c.sql"];
  assert.deepEqual(stamps.sort(byName).map(fileName), ["002_c.sql", "20240115_a.sql", "20240201_b.sql"]);
  // The case of a name does not order it
  assert.ok(byName("/a/v3__x.sql", "/a/V20__y.sql") < 0);
});

test("files added go to their place by name, each once", () => {
  let q = addFiles(emptyQueue, ["/m/003.sql", "/m/001.sql"]);
  q = addFiles(q, ["/m/002.sql", "/m/001.sql", "/m/002.sql"]);
  assert.deepEqual(names(q), ["001.sql", "002.sql", "003.sql"]);
  assert.ok(q.items.every((i) => i.status === "pending"));
});

test("once moved, the queue keeps the user's order and files added go last", () => {
  let q = addFiles(emptyQueue, ["/m/001.sql", "/m/002.sql", "/m/003.sql"]);
  q = move(q, 2, 0);
  assert.deepEqual(names(q), ["003.sql", "001.sql", "002.sql"]);
  assert.equal(q.byName, false);
  q = addFiles(q, ["/m/000.sql", "/m/004.sql"]);
  assert.deepEqual(names(q), ["003.sql", "001.sql", "002.sql", "000.sql", "004.sql"]);
  q = sortByName(q);
  assert.deepEqual(names(q), ["000.sql", "001.sql", "002.sql", "003.sql", "004.sql"]);
  assert.equal(q.byName, true);
  // Moved back into the order of the names, it is in it again
  q = move(move(q, 0, 1), 1, 0);
  assert.equal(q.byName, true);
});

test("the file running is neither moved nor removed", () => {
  let q = addFiles(emptyQueue, ["/m/001.sql", "/m/002.sql", "/m/003.sql"]);
  q = patchItem(q, "/m/002.sql", { status: "running" });
  assert.equal(move(q, 1, 0), q);
  assert.equal(move(q, 0, 1), q);
  assert.equal(remove(q, "/m/002.sql"), q);
  assert.deepEqual(names(remove(q, "/m/003.sql")), ["001.sql", "002.sql"]);
  assert.deepEqual(names(move(q, 2, 0)), ["003.sql", "001.sql", "002.sql"]);
});

test("the next file is the first not run through, a failed one again", () => {
  let q = addFiles(emptyQueue, ["/m/001.sql", "/m/002.sql", "/m/003.sql"]);
  assert.equal(fileName(nextItem(q).path), "001.sql");
  q = patchItem(q, "/m/001.sql", { status: "done" });
  q = patchItem(q, "/m/002.sql", { status: "failed", note: "1 failed" });
  assert.equal(fileName(nextItem(q).path), "002.sql");
  q = patchItem(q, "/m/002.sql", { status: "done" });
  q = patchItem(q, "/m/003.sql", { status: "done" });
  assert.equal(nextItem(q), undefined);
  assert.deepEqual(names(clearDone(q)), []);
  assert.ok(reset(q).items.every((i) => i.status === "pending" && i.note === undefined));
});
