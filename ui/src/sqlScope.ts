// What a statement reads from and what it calls each source: enough of SQL
// to complete `p.` after `from PRODUTO p`, and no more. It reads text that is
// being typed, so it never fails: what it does not follow it leaves out.
//
// No imports: `npm test` runs this file as it is.

/** A relation a statement names, and the alias it gives it */
export interface Source {
  /** As the catalog stores it */
  relation: string;
  alias?: string;
  /** What the statement calls it, as it wrote it: the alias, else the name */
  written: string;
}

const NAME = String.raw`"(?:[^"]|"")+"|[A-Za-z_][\w$]*`;
const LEAD = /\b(from|join|update|into|using)\b/gi;
const ITEM = new RegExp(String.raw`\s*(${NAME})`, "y");
const ALIAS = new RegExp(String.raw`\s+(?:as\s+)?(${NAME})`, "iy");

/** What follows a source and is not its alias */
const NOT_A_NAME = new Set(
  `as cross default do fetch for full group having inner into join lateral left matched natural offset on
   or order plan returning right rows select set union using values when where window with`.split(/\s+/),
);

/** A name as the catalog stores it: as written when quoted, upper case when not */
export function stored(written: string): string {
  return written.startsWith('"') ? written.slice(1, written.endsWith('"') && written.length > 1 ? -1 : undefined).replaceAll('""', '"') : written.toUpperCase();
}

/** The text with its strings and comments blanked, every position kept */
export function blanked(text: string): string {
  return text.replace(/'(?:[^']|'')*'?|--[^\n]*|\/\*[\s\S]*?(?:\*\/|$)|"(?:[^"]|"")*"?/g, (m) =>
    m.startsWith('"') ? m : m.replace(/[^\n]/g, " "),
  );
}

/** Where the statement around `pos` starts and ends, in blanked text. In
 *  PSQL that is the statement inside the body, which is the one whose
 *  aliases are in reach. */
export function statementAt(text: string, pos: number): { from: number; to: number } {
  const from = text.lastIndexOf(";", pos - 1) + 1;
  const end = text.indexOf(";", pos);
  return { from, to: end < 0 ? text.length : end };
}

function closing(text: string, open: number): number {
  let depth = 0;
  for (let i = open; i < text.length; i++) {
    if (text[i] === "(") depth++;
    else if (text[i] === ")" && --depth === 0) return i + 1;
  }
  return -1;
}

/** The sources of the statement around `pos` */
export function sources(text: string, pos: number): Source[] {
  const clean = blanked(text);
  const { from, to } = statementAt(clean, pos);
  const statement = clean.slice(from, to);
  const found: Source[] = [];

  LEAD.lastIndex = 0;
  for (let lead = LEAD.exec(statement); lead; lead = LEAD.exec(statement)) {
    let at = LEAD.lastIndex;
    // FROM takes a list: `from A x, B y`
    for (;;) {
      ITEM.lastIndex = at;
      const item = ITEM.exec(statement);
      if (!item || NOT_A_NAME.has(item[1].toLowerCase())) break;
      at = ITEM.lastIndex;
      // A selectable procedure's arguments, or an INSERT's columns
      const paren = /\s*\(/y;
      paren.lastIndex = at;
      if (paren.test(statement)) {
        at = closing(statement, paren.lastIndex - 1);
        if (at < 0) {
          found.push({ relation: stored(item[1]), written: item[1] });
          break;
        }
      }
      const source: Source = { relation: stored(item[1]), written: item[1] };
      ALIAS.lastIndex = at;
      const alias = ALIAS.exec(statement);
      if (alias && !NOT_A_NAME.has(alias[1].toLowerCase())) {
        source.alias = stored(alias[1]);
        source.written = alias[1];
        at = ALIAS.lastIndex;
      }
      found.push(source);
      const comma = /\s*,/y;
      comma.lastIndex = at;
      if (lead[1].toLowerCase() !== "from" || !comma.test(statement)) break;
      at = comma.lastIndex;
    }
    LEAD.lastIndex = Math.max(at, LEAD.lastIndex);
  }
  return found;
}

/** The name before the dot that ends `before`, as the catalog stores it */
export function qualifier(before: string): string | null {
  const m = new RegExp(String.raw`(${NAME})\s*\.\s*$`).exec(before);
  return m ? stored(m[1]) : null;
}

/** A foreign key: `from` holds it, `to` has the key it points at */
export interface Reference {
  from: string;
  fromColumns: string[];
  to: string;
  toColumns: string[];
}

/** A join a foreign key offers to a statement */
export interface Join {
  /** The relation it brings in, as the catalog stores it */
  relation: string;
  /** The alias it is given, which no source of the statement has */
  alias: string;
  /** Column pairs: the new relation's, then the one of `with` */
  on: [string, string][];
  /** The source it joins to */
  with: Source;
}

/** Short words an alias must not be: they would read as SQL */
const NOT_AN_ALIAS = new Set("as at by do if in is no of on or to".split(" "));

/**
 * An alias for a relation: the initials of its words (`CLIENTE_PESSOA` is
 * `cp`), numbered when a source of the statement already answers to it. In
 * capitals when `upper`.
 */
export function aliasFor(relation: string, taken: Iterable<string>, upper = false): string {
  const used = new Set([...taken].map((name) => name.toUpperCase()));
  const initials = relation
    .split(/[^A-Za-z0-9]+/)
    .filter((part) => /^[A-Za-z]/.test(part))
    .map((part) => part[0])
    .join("")
    .slice(0, 4)
    .toLowerCase();
  const base = initials || "t";
  let alias = base;
  for (let n = 2; used.has(alias.toUpperCase()) || NOT_AN_ALIAS.has(alias); n++) alias = `${base}${n}`;
  return upper ? alias.toUpperCase() : alias;
}

/**
 * The joins the foreign keys offer to what a statement already reads: for
 * each of its sources, the tables that reference it and the ones it
 * references. A table two keys lead to is offered once per key.
 */
export function joinsFor(inStatement: Source[], references: Reference[]): Join[] {
  const taken = inStatement.map((s) => s.alias ?? s.relation);
  // Written like the statement: capitals when all it wrote is in capitals
  const letters = inStatement.map((s) => s.written).filter((w) => /[A-Za-z]/.test(w) && !w.startsWith('"'));
  const upper = letters.length > 0 && letters.every((w) => w === w.toUpperCase());
  const joins: Join[] = [];
  const seen = new Set<string>();
  for (const source of inStatement) {
    const key = source.alias ?? source.relation;
    if (seen.has(key)) continue;
    seen.add(key);
    for (const r of references) {
      const offer = (relation: string, own: string[], theirs: string[]) =>
        joins.push({
          relation,
          alias: aliasFor(relation, taken, upper),
          on: own.map((column, i): [string, string] => [column, theirs[i]]),
          with: source,
        });
      if (r.to === source.relation) offer(r.from, r.fromColumns, r.toColumns);
      if (r.from === source.relation && r.to !== r.from) offer(r.to, r.toColumns, r.fromColumns);
    }
  }
  return joins;
}
