// Run by `npm test`: Node's own runner, reading the TypeScript as it is.
import assert from "node:assert/strict";
import { test } from "node:test";
import { COMMANDS, commandsFor, conflicts, display, normalize, pressed } from "./keymap.ts";

const press = (key, code, mods = {}) => ({
  key,
  code,
  ctrlKey: false,
  altKey: false,
  shiftKey: false,
  metaKey: false,
  ...mods,
});

test("a key is written one way whatever the order and the names of its modifiers", () => {
  assert.equal(normalize("Shift-Mod-f9", false), "Ctrl-Shift-F9");
  assert.equal(normalize("Mod-Shift-F9", true), "Shift-Meta-F9");
  assert.equal(normalize("cmd-s", true), "Meta-S");
  assert.equal(normalize("Ctrl--", false), "Ctrl--");
  assert.equal(normalize("Ctrl-", false), null);
  assert.equal(normalize("Ctrl-Shift", false), null);
  assert.equal(normalize("Hyper-A", false), null);
});

test("a key press is the letter typed, or with Alt the key where it is", () => {
  assert.equal(pressed(press("s", "KeyS", { ctrlKey: true })), "Ctrl-S");
  // A layout that has the letter elsewhere: the letter, not the place
  assert.equal(pressed(press("z", "KeyW", { ctrlKey: true })), "Ctrl-Z");
  // Option+F types ƒ on a Mac, AltGr+C ₢ on a Brazilian keyboard
  assert.equal(pressed(press("ƒ", "KeyF", { altKey: true })), "Alt-F");
  assert.equal(pressed(press("₢", "KeyC", { ctrlKey: true, altKey: true })), "Ctrl-Alt-C");
  // A Cyrillic letter: the place
  assert.equal(pressed(press("ы", "KeyS", { ctrlKey: true })), "Ctrl-S");
  assert.equal(pressed(press("!", "Digit1", { shiftKey: true })), "Shift-1");
  assert.equal(pressed(press(" ", "Space", { ctrlKey: true })), "Ctrl-Space");
  assert.equal(pressed(press("Enter", "NumpadEnter", { metaKey: true })), "Meta-Enter");
  assert.equal(pressed(press("F9", "F9", { shiftKey: true })), "Shift-F9");
  assert.equal(pressed(press(",", "Comma", { metaKey: true })), "Meta-Comma");
  assert.equal(pressed(press("Control", "ControlLeft", { ctrlKey: true })), null);
});

test("a key is found in its place, then in SQL text, then everywhere", () => {
  const ids = (key, scope, overrides = {}, mac = false) => commandsFor(key, scope, overrides, mac).map((c) => c.id);
  assert.deepEqual(ids("F9", "editor"), ["editor.run"]);
  assert.deepEqual(ids("F9", "viewer"), ["viewer.compile"]);
  assert.deepEqual(ids("Shift-F9", "executive"), ["executive.runFromFile"]);
  assert.deepEqual(ids("Ctrl-Space", "viewer"), ["text.complete"]);
  assert.deepEqual(ids("F12", "editor"), ["app.newEditor"]);
  assert.deepEqual(ids("Meta-Enter", "editor", {}, true), ["editor.run"]);
  assert.deepEqual(ids("Ctrl-Enter", "editor", {}, true), []);
  // The user's keys take the place of the defaults
  assert.deepEqual(ids("F9", "editor", { "editor.run": ["F5"] }), []);
  assert.deepEqual(ids("F5", "editor", { "editor.run": ["F5"] }), ["editor.run"]);
});

test("the defaults run one command per key wherever they can be pressed", () => {
  for (const mac of [false, true]) {
    for (const c of COMMANDS) {
      for (const k of c.keys) assert.deepEqual(conflicts(c.id, k, {}, mac), [], `${c.id} ${k} mac=${mac}`);
    }
  }
});

test("a key given to two commands that work in one place is a conflict", () => {
  const overrides = { "editor.save": ["F9"] };
  assert.deepEqual(
    conflicts("editor.save", "F9", overrides, false).map((c) => c.id),
    ["editor.run"],
  );
  // The object editor's F9 is not pressed where the SQL editor's is
  assert.deepEqual(conflicts("viewer.compile", "F9", overrides, false), []);
  // An app key is pressed everywhere
  assert.deepEqual(
    conflicts("app.newEditor", "F9", { "app.newEditor": ["F9"] }, false).map((c) => c.id),
    ["editor.run", "viewer.compile", "executive.run"],
  );
});

test("a key is shown as the keyboard labels it", () => {
  assert.equal(display("Mod-Shift-S", false), "Ctrl+Shift+S");
  assert.equal(display("Mod-Shift-S", true), "⇧⌘S");
  assert.equal(display("Ctrl-Alt-C", true), "⌃⌥C");
  assert.equal(display("Mod-Comma", false), "Ctrl+,");
  assert.equal(display("Escape", false), "Esc");
  assert.equal(display("Ctrl-PageDown", false), "Ctrl+Page Down");
});
