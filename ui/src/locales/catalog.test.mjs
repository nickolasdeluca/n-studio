// Run by `npm test`: Node's own runner, reading the TypeScript as it is.
// Keeps the pt-BR catalog in step with the code: every t("…") of the UI has
// an entry, every entry is a text the code still has, and each keeps the
// {placeholders} of its English.
import assert from "node:assert/strict";
import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { ptBR } from "./pt-BR.ts";

const here = fileURLToPath(new URL(".", import.meta.url));
const root = join(here, "..", "..", "..");

function files(dir, ext) {
  return readdirSync(dir, { recursive: true, withFileTypes: true })
    .filter((e) => e.isFile() && ext.test(e.name) && !/\.test\.|\.tmp\.|^pt-BR\.ts$/.test(e.name))
    .map((e) => join(e.parentPath, e.name));
}

const ui = files(join(root, "ui", "src"), /\.(ts|tsx)$/).map((f) => readFileSync(f, "utf8"));
const rust = [join(root, "src-tauri", "src"), ...readdirSync(join(root, "crates")).map((c) => join(root, "crates", c, "src"))]
  .flatMap((d) => files(d, /\.rs$/))
  .map((f) => readFileSync(f, "utf8"));

/** A JS or Rust string literal's text, its escapes undone */
const unescape = (s) => s.replace(/\\(["\\nt])/g, (_, c) => (c === "n" ? "\n" : c === "t" ? "\t" : c));

test("every text the UI translates is in the catalog", () => {
  const missing = new Set();
  for (const source of ui) {
    for (const m of source.matchAll(/\bt\(\s*"((?:[^"\\]|\\.)*)"/g)) {
      const text = unescape(m[1]);
      if (!(text in ptBR)) missing.add(text);
    }
    for (const m of source.matchAll(/\btx\(\s*"(\w+)",\s*"((?:[^"\\]|\\.)*)"/g)) {
      const key = `${unescape(m[2])} [${m[1]}]`;
      if (!(key in ptBR)) missing.add(key);
    }
  }
  assert.deepEqual([...missing], []);
});

test("every text in the catalog is one the code still has", () => {
  const all = [...ui, ...rust].join("\n");
  // As a literal is written: in double quotes, its own quotes escaped
  const written = (k) => `"${k.replace(/\\/g, "\\\\").replace(/"/g, '\\"').replace(/\n/g, "\\n")}"`;
  assert.deepEqual(
    Object.keys(ptBR).filter((k) => {
      // "text [context]" is written tx("context", "text")
      const m = /^(.*) \[(\w+)\]$/s.exec(k);
      return !(m ? all.includes(`tx("${m[2]}", ${written(m[1])}`) : all.includes(written(k)));
    }),
    [],
  );
});

test("a translation keeps the placeholders of its English", () => {
  const names = (s) => [...s.matchAll(/\{(\w+)\}/g)].map((m) => m[1]).sort();
  for (const [english, said] of Object.entries(ptBR)) {
    assert.ok(said.trim() !== "" || english.trim() === "", english);
    assert.deepEqual(names(said), names(english), english);
  }
});
