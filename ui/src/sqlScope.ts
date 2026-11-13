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
          found.push({ relation: stored(item[1]) });
          break;
        }
      }
      const source: Source = { relation: stored(item[1]) };
      ALIAS.lastIndex = at;
      const alias = ALIAS.exec(statement);
      if (alias && !NOT_A_NAME.has(alias[1].toLowerCase())) {
        source.alias = stored(alias[1]);
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
