// The app's commands and the keys that run them: IBExpert's keys by default,
// so whoever comes from it keeps them, and any of them changed in the
// preferences. A key is written as CodeMirror writes one ("Mod-Shift-F9"),
// Mod being Cmd on a Mac and Ctrl elsewhere. Nothing here reads the browser,
// so the tests run it as it is.

/** Where a command's keys work. The app's work everywhere, and SQL text's in
 *  every editor of SQL: a key of theirs is a key of every other scope too. */
export type Scope = "app" | "text" | "editor" | "viewer" | "executive";

export interface Command {
  id: string;
  label: string;
  scope: Scope;
  /** IBExpert's key first, where it has one */
  keys: string[];
}

export const SCOPES: [Scope, string][] = [
  ["app", "Everywhere"],
  ["text", "Any SQL text"],
  ["editor", "SQL Editor"],
  ["viewer", "Object editor"],
  ["executive", "Script Executive"],
];

export const COMMANDS: Command[] = [
  { id: "app.newEditor", label: "New SQL Editor", scope: "app", keys: ["F12"] },
  { id: "app.openScript", label: "Open Script…", scope: "app", keys: ["Mod-O"] },
  { id: "app.executive", label: "Script Executive", scope: "app", keys: ["Mod-F12", "Ctrl-F12"] },
  { id: "app.preferences", label: "Preferences…", scope: "app", keys: ["Mod-Comma"] },

  { id: "text.complete", label: "Complete", scope: "text", keys: ["Ctrl-Space"] },

  { id: "editor.run", label: "Execute", scope: "editor", keys: ["F9", "Mod-Enter"] },
  { id: "editor.runAll", label: "Execute All", scope: "editor", keys: ["Shift-F9", "Mod-Shift-Enter"] },
  { id: "editor.runToCursor", label: "Execute to Cursor", scope: "editor", keys: ["Alt-F9", "Mod-Alt-Enter"] },
  { id: "editor.runScript", label: "Execute as Script", scope: "editor", keys: ["Ctrl-F9"] },
  { id: "editor.stop", label: "Stop", scope: "editor", keys: ["Escape"] },
  { id: "editor.commit", label: "Commit", scope: "editor", keys: ["Mod-Alt-C", "Ctrl-Alt-C"] },
  { id: "editor.rollback", label: "Rollback", scope: "editor", keys: ["Mod-Alt-R", "Ctrl-Alt-R"] },
  { id: "editor.format", label: "Format", scope: "editor", keys: ["Alt-F", "Mod-Shift-F"] },
  { id: "editor.save", label: "Save", scope: "editor", keys: ["Mod-S"] },
  { id: "editor.saveAs", label: "Save As…", scope: "editor", keys: ["Mod-Shift-S"] },
  { id: "editor.newPage", label: "New Page", scope: "editor", keys: ["Mod-T"] },
  { id: "editor.nextPage", label: "Next Page", scope: "editor", keys: ["Ctrl-Tab", "Ctrl-PageDown"] },
  { id: "editor.previousPage", label: "Previous Page", scope: "editor", keys: ["Ctrl-Shift-Tab", "Ctrl-PageUp"] },

  { id: "viewer.compile", label: "Compile", scope: "viewer", keys: ["F9", "Mod-Enter"] },

  { id: "executive.run", label: "Run", scope: "executive", keys: ["F9"] },
  { id: "executive.runFromFile", label: "Run from File…", scope: "executive", keys: ["Shift-F9"] },
  { id: "executive.save", label: "Save", scope: "executive", keys: ["Mod-S"] },
];

const byId = new Map(COMMANDS.map((c) => [c.id, c]));

export const command = (id: string) => byId.get(id);

/** Modifiers in the order a key is written with them */
const MODIFIERS = ["Ctrl", "Alt", "Shift", "Meta"] as const;
type Modifier = (typeof MODIFIERS)[number];

/** A key as one string, its modifiers in one order and Mod made Cmd or
 *  Ctrl: two ways of writing the same key are the same string. Null when it
 *  is no key ("Shift-", "Ctrl-Ctrl"). */
export function normalize(key: string, mac: boolean): string | null {
  const parts = key.split("-");
  // "Ctrl--" is Ctrl with the minus key
  if (parts.length > 1 && parts[parts.length - 1] === "" && parts[parts.length - 2] === "") parts.splice(-2, 2, "-");
  const name = parts.pop();
  if (!name) return null;
  const held = new Set<Modifier>();
  for (const p of parts) {
    const m = p.toLowerCase();
    if (m === "mod") held.add(mac ? "Meta" : "Ctrl");
    else if (m === "ctrl" || m === "control" || m === "c") held.add("Ctrl");
    else if (m === "alt" || m === "option" || m === "a") held.add("Alt");
    else if (m === "shift" || m === "s") held.add("Shift");
    else if (m === "meta" || m === "cmd" || m === "m") held.add("Meta");
    else return null;
  }
  if (MODIFIERS.includes(name as Modifier)) return null;
  const main =
    name.length === 1 || /^f\d{1,2}$/i.test(name) ? name.toUpperCase() : name[0].toUpperCase() + name.slice(1);
  return [...MODIFIERS.filter((m) => held.has(m)), main].join("-");
}

/** What a key event is, enough to say which key it is */
export interface KeyPress {
  key: string;
  code: string;
  ctrlKey: boolean;
  altKey: boolean;
  shiftKey: boolean;
  metaKey: boolean;
}

/** The key a keyboard's key is, by where it is, for when what it types
 *  says nothing (Option+F types "ƒ" on a Mac, AltGr+C "₢" on a Brazilian
 *  keyboard). Null for a modifier on its own. */
function byCode(code: string): string | null {
  if (/^Key[A-Z]$/.test(code)) return code.slice(3);
  if (/^Digit\d$/.test(code)) return code.slice(5);
  if (/^(Shift|Control|Alt|Meta|OS)(Left|Right)?$/.test(code) || code === "CapsLock" || code === "Fn") return null;
  if (code === "NumpadEnter") return "Enter";
  return code || null;
}

/** A key event as a key, normalized as `normalize` writes one. A letter or
 *  digit is the one typed, so a key is where the keyboard has it; with Alt,
 *  which makes them type something else, the one where it is. */
export function pressed(e: KeyPress): string | null {
  const typed = e.key.length === 1 && /[a-z0-9]/i.test(e.key) && !e.altKey ? e.key.toUpperCase() : null;
  const main = typed ?? (e.key === " " ? "Space" : byCode(e.code));
  if (!main) return null;
  const held: Modifier[] = [];
  if (e.ctrlKey) held.push("Ctrl");
  if (e.altKey) held.push("Alt");
  if (e.shiftKey) held.push("Shift");
  if (e.metaKey) held.push("Meta");
  return [...held, main].join("-");
}

/** The keys a command has: the user's, or its defaults */
export function keysOf(id: string, overrides: Record<string, string[]>): string[] {
  return overrides[id] ?? byId.get(id)?.keys ?? [];
}

/** Whether keys of one scope can be pressed where keys of the other work */
export function overlaps(a: Scope, b: Scope): boolean {
  return a === b || a === "app" || b === "app" || a === "text" || b === "text";
}

/** The scopes a key is looked for in, from a place of `scope`: its own,
 *  then SQL text's when it is SQL text, then the app's */
function reach(scope: Scope): Scope[] {
  return scope === "app" ? ["app"] : scope === "text" ? ["text", "app"] : [scope, "text", "app"];
}

/** The commands a key runs where `scope`'s keys work, the nearest scope's
 *  first: the place's own, SQL text's, the app's */
export function commandsFor(
  key: string,
  scope: Scope,
  overrides: Record<string, string[]>,
  mac: boolean,
): Command[] {
  const found: Command[] = [];
  for (const s of reach(scope)) {
    for (const c of COMMANDS) {
      if (c.scope === s && keysOf(c.id, overrides).some((k) => normalize(k, mac) === key)) found.push(c);
    }
  }
  return found;
}

/** The other commands a key of a command also runs, where both work */
export function conflicts(id: string, key: string, overrides: Record<string, string[]>, mac: boolean): Command[] {
  const own = byId.get(id);
  const wanted = normalize(key, mac);
  if (!own || !wanted) return [];
  return COMMANDS.filter(
    (c) =>
      c.id !== id &&
      overlaps(c.scope, own.scope) &&
      keysOf(c.id, overrides).some((k) => normalize(k, mac) === wanted),
  );
}

const GLYPHS: Record<Modifier, string> = { Ctrl: "⌃", Alt: "⌥", Shift: "⇧", Meta: "⌘" };
const NAMES: Record<string, string> = {
  Comma: ",",
  Period: ".",
  Slash: "/",
  Backslash: "\\",
  Semicolon: ";",
  Quote: "'",
  BracketLeft: "[",
  BracketRight: "]",
  Minus: "-",
  Equal: "=",
  Backquote: "`",
  ArrowUp: "↑",
  ArrowDown: "↓",
  ArrowLeft: "←",
  ArrowRight: "→",
  Escape: "Esc",
  PageUp: "Page Up",
  PageDown: "Page Down",
};

/** A key as the keyboard's labels say it: ⌘⇧S on a Mac, Ctrl+Shift+S
 *  elsewhere */
export function display(key: string, mac: boolean): string {
  const k = normalize(key, mac);
  if (!k) return key;
  const parts = k.split("-");
  const main = parts.pop() ?? "";
  const name = NAMES[main] ?? main;
  if (mac) return parts.map((p) => GLYPHS[p as Modifier]).join("") + name;
  return [...parts.map((p) => (p === "Meta" ? "Win" : p)), name].join("+");
}
