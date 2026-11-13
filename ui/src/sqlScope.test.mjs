// Run by `npm test`: Node's own runner, reading the TypeScript as it is.
import assert from "node:assert/strict";
import { test } from "node:test";
import { blanked, qualifier, sources, statementAt, stored } from "./sqlScope.ts";

const at = (sql) => {
  const pos = sql.indexOf("|");
  return sources(sql.replace("|", ""), pos < 0 ? sql.length : pos);
};

test("a bare name is stored in upper case, a quoted one as written", () => {
  assert.equal(stored("produto"), "PRODUTO");
  assert.equal(stored('"Produto"'), "Produto");
  assert.equal(stored('"a""b"'), 'a"b');
  // Still being typed
  assert.equal(stored('"Prod'), "Prod");
});

test("tables and their aliases, with AS and without", () => {
  assert.deepEqual(at("select p.| from produto p join ncm as n on n.id = p.ncm"), [
    { relation: "PRODUTO", alias: "P" },
    { relation: "NCM", alias: "N" },
  ]);
});

test("a clause's word is not an alias", () => {
  assert.deepEqual(at("select | from produto where id = 1"), [{ relation: "PRODUTO" }]);
  assert.deepEqual(at("select * from a left join b on b.id = a.id order by 1"), [{ relation: "A" }, { relation: "B" }]);
  assert.deepEqual(at("select * from a as"), [{ relation: "A" }]);
});

test("FROM takes a list", () => {
  assert.deepEqual(at("select * from pedido p, pedido_item i, tipos where p.id = i.pedido"), [
    { relation: "PEDIDO", alias: "P" },
    { relation: "PEDIDO_ITEM", alias: "I" },
    { relation: "TIPOS" },
  ]);
});

test("update, insert, delete and merge name their table", () => {
  assert.deepEqual(at("update produto p set p.| = 1"), [{ relation: "PRODUTO", alias: "P" }]);
  assert.deepEqual(at("insert into produto (id, nome) values (1, 'x')"), [{ relation: "PRODUTO" }]);
  assert.deepEqual(at("delete from produto p where p.id = 1"), [{ relation: "PRODUTO", alias: "P" }]);
  assert.deepEqual(at("merge into alvo a using origem o on a.id = o.id"), [
    { relation: "ALVO", alias: "A" },
    { relation: "ORIGEM", alias: "O" },
  ]);
});

test("a selectable procedure's arguments are stepped over", () => {
  assert.deepEqual(at("select e.| from p_even(4, (select 1 from rdb$database)) e"), [{ relation: "P_EVEN", alias: "E" }]);
  // Unfinished
  assert.deepEqual(at("select * from p_even(4"), [{ relation: "P_EVEN" }]);
});

test("quoted names keep their case and may hold anything", () => {
  assert.deepEqual(at('select * from "Pedido Item" "i" join "from" f on 1 = 1'), [
    { relation: "Pedido Item", alias: "i" },
    { relation: "from", alias: "F" },
  ]);
});

test("strings and comments are not read", () => {
  assert.deepEqual(at("select 'from x y' from a /* join b c */ -- from d\n where 1 = 1"), [{ relation: "A" }]);
  assert.equal(blanked("a 'b;c' -- d;\ne").length, "a 'b;c' -- d;\ne".length);
});

test("only the statement around the cursor counts", () => {
  assert.deepEqual(at("select * from a x; select x.| from b x; select * from c x"), [{ relation: "B", alias: "X" }]);
  assert.deepEqual(at("select ';' from a; select | from b"), [{ relation: "B" }]);
  assert.deepEqual(statementAt("a; b; c", 4), { from: 2, to: 4 });
});

test("PSQL: the statement inside the body, and INTO's variables are no source", () => {
  const sql = "create procedure p as begin for select i.| from pedido_item i into :a, :b do suspend; end";
  assert.deepEqual(at(sql), [{ relation: "PEDIDO_ITEM", alias: "I" }]);
});

test("the name before a dot", () => {
  assert.equal(qualifier("select p."), "P");
  assert.equal(qualifier('select "Pedido Item" . '), "Pedido Item");
  assert.equal(qualifier("select rdb$relations."), "RDB$RELATIONS");
  assert.equal(qualifier("select p"), null);
  assert.equal(qualifier("select 1."), null);
});
