import assert from "node:assert/strict";
import { test } from "node:test";
import { selectEditing, tableOf } from "./selectEdit.ts";

const column = (name, relation = "PRODUTO", field = name, kind = "text") => ({ name, relation, field, kind, typeName: "", nullable: true });
const editing = { key: ["ID"], editable: ["ID", "NOME", "PRECO"] };

test("a select of one table is that table's rows", () => {
  const columns = [column("ID", "PRODUTO", "ID", "integer"), column("NOME"), column("TOTAL", "", "", "decimal")];
  assert.equal(tableOf(columns, ["PRODUTO"]), "PRODUTO");
  // Joined to another table of which it shows nothing, it still is
  assert.equal(tableOf(columns, ["PRODUTO", "GRUPO"]), "PRODUTO");
});

test("columns of two tables, or a table read twice, are nobody's rows", () => {
  assert.equal(tableOf([column("ID"), column("NOME", "GRUPO")], ["PRODUTO", "GRUPO"]), null);
  assert.equal(tableOf([column("ID"), column("NOME")], ["PRODUTO", "PRODUTO"]), null);
  assert.equal(tableOf([column("N", "", "")], ["PRODUTO"]), null);
});

test("the key has to be among the columns", () => {
  assert.equal(selectEditing([column("NOME")], "PRODUTO", editing), null);
  // A key that cannot be typed back, as a blob's, tells no row apart
  assert.equal(selectEditing([column("ID", "PRODUTO", "ID", "blob")], "PRODUTO", editing), null);
});

test("columns are the table's whatever they are headed with", () => {
  const columns = [
    column("CODIGO", "PRODUTO", "ID", "integer"),
    column("NOME"),
    column("DOBRO", "", "", "decimal"),
    column("OUTRA_VEZ", "PRODUTO", "NOME"),
    column("FOTO", "PRODUTO", "FOTO", "blob"),
    column("CALCULADO", "PRODUTO", "CALCULADO", "decimal"),
  ];
  const e = selectEditing(columns, "PRODUTO", editing);
  assert.deepEqual(e.key, [0]);
  // An expression, a column come twice, a blob and a computed column are read only
  assert.deepEqual(e.editable, [true, true, false, false, false, false]);
  assert.deepEqual(e.names.slice(0, 2), ["ID", "NOME"]);
  assert.notEqual(e.names[2], e.names[3]);
});
