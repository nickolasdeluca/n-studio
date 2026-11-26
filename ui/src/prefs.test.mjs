// Run by `npm test`: Node's own runner, reading the TypeScript as it is.
import assert from "node:assert/strict";
import { test } from "node:test";
import { FONT_DEFAULTS, localeOf, readPrefs, systemLocale } from "./prefs.ts";

test("nothing stored, or something broken, is the defaults", () => {
  const defaults = { keys: {}, fonts: FONT_DEFAULTS, language: "system" };
  assert.deepEqual(readPrefs(null), defaults);
  assert.deepEqual(readPrefs("{not json"), defaults);
  assert.deepEqual(readPrefs("[1,2]"), { ...defaults, keys: {} });
});

test("what was stored is kept as far as it makes sense", () => {
  const p = readPrefs(
    JSON.stringify({
      keys: { "editor.run": ["F5", 7], "editor.save": "Mod-S", "editor.stop": [] },
      fonts: { zoom: 500, editor: 15.4, grid: "big" },
      language: "pt-BR",
    }),
  );
  assert.deepEqual(p.keys, { "editor.run": ["F5"], "editor.stop": [] });
  assert.deepEqual(p.fonts, { zoom: 200, editor: 15, grid: FONT_DEFAULTS.grid });
  assert.equal(p.language, "pt-BR");
  assert.equal(readPrefs(JSON.stringify({ language: "klingon" })).language, "system");
});

test("the system's language is the first of its languages there is a catalog for", () => {
  assert.equal(systemLocale(["pt-BR", "en-US"]), "pt-BR");
  assert.equal(systemLocale(["pt-PT"]), "pt-BR");
  assert.equal(systemLocale(["de-DE", "en-GB", "pt-BR"]), "en");
  assert.equal(systemLocale(["de-DE", "pt"]), "pt-BR");
  assert.equal(systemLocale(["ja"]), "en");
  assert.equal(systemLocale([]), "en");
  assert.equal(localeOf("system", ["pt-BR"]), "pt-BR");
  assert.equal(localeOf("en", ["pt-BR"]), "en");
});
