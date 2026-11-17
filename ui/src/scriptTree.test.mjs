import assert from "node:assert/strict";
import { test } from "node:test";
import { scriptTree, treeKinds } from "./scriptTree.ts";

let index = 0;
const block = (operation, kind, name = null, member = null) => ({
  index: index++,
  from: 0,
  to: 0,
  line: 1,
  lines: 1,
  kind: "sql",
  subject: { operation, kind, name, member },
  summary: "",
});

test("a script's statements go under their objects, by kind", () => {
  index = 0;
  const tree = scriptTree([
    block("alter", "table", "PARAMETRO"),
    block("comment", "table", "PARAMETRO", "COLUNA"),
    block("create", "index", "IX_A", "PARAMETRO"),
    block("alter", "procedure", "P"),
    block("createOrAlter", "trigger", "T_BI", "PARAMETRO"),
    block("execute", "block"),
    block("commit", null),
    block("alter", "procedure", "P"),
  ]);
  assert.deepEqual(tree.map((g) => g.kind), treeKinds);
  const of = (kind) => tree.find((g) => g.kind === kind);

  const tables = of("table").objects;
  assert.equal(tables.length, 1);
  assert.equal(tables[0].name, "PARAMETRO");
  assert.deepEqual(
    tables[0].items.map((i) => [i.index, i.operation, i.detail, i.index_on]),
    [
      [0, "alter", null, false],
      [1, "comment", ".COLUNA", false],
      [2, "create", "IX_A", true],
    ],
  );
  // An index is under Indices too; a trigger only under Triggers
  assert.deepEqual(of("index").objects.map((o) => o.name), ["IX_A"]);
  assert.deepEqual(of("trigger").objects.map((o) => [o.name, o.items.length]), [["T_BI", 1]]);
  // One procedure altered twice is one object with two statements
  assert.deepEqual(of("procedure").objects.map((o) => [o.name, o.items.map((i) => i.index)]), [["P", [3, 7]]]);
  // A block has no name; COMMIT has no kind and is not in the tree
  assert.deepEqual(of("block").loose.map((i) => i.index), [5]);
  assert.equal(tree.flatMap((g) => [...g.loose, ...g.objects.flatMap((o) => o.items)]).filter((i) => i.index === 6).length, 0);
  // Kinds with nothing are there, empty
  assert.deepEqual(of("view"), { kind: "view", objects: [], loose: [] });
});
