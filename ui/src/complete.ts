// Completion of a database's own names in SQL: relations and what else a
// statement names, a relation's columns after its name or its alias and a
// dot, the columns of what the statement already reads from, and after
// JOIN the joins its foreign keys offer, written whole with an alias.
// Keywords are the language's own source (firebird.ts).
import { autocompletion, type Completion, type CompletionContext, type CompletionResult } from "@codemirror/autocomplete";
import { syntaxTree } from "@codemirror/language";
import type { Extension } from "@codemirror/state";
import { api, type CatalogRelation, type CompletionCatalog, type DiagramReference, type ObjectKind, type Word } from "./api";
import { firebirdDialect } from "./firebird";
import { t } from "./i18n";
import { joinsFor, qualifier, sources } from "./sqlScope";
import { useStore } from "./store";

interface Names {
  relations: Map<string, CatalogRelation>;
  /** Everything that completes where no relation is named before a dot */
  top: Completion[];
  columns: Map<CatalogRelation, Completion[]>;
  references: DiagramReference[];
}

/** By registration. Read when an editor first asks, and again after the
 *  explorer's tree is: that is what a refresh and a changed catalog do. */
const loaded = new Map<string, Promise<Names | null>>();

useStore.subscribe((state, before) => {
  for (const id of loaded.keys()) {
    const tree = state.trees[id];
    if (!tree || (tree.loading && !before.trees[id]?.loading)) loaded.delete(id);
  }
});

/** What a completion is, said in the list's first column. The language's
 *  own source calls a keyword "keyword", a type "type" and a built-in
 *  function "variable". */
const KINDS: Record<string, string> = {
  table: t("Table"),
  view: t("View"),
  system: t("System"),
  column: t("Column"),
  alias: t("Alias"),
  join: t("Join"),
  procedure: t("Procedure"),
  function: t("Function"),
  package: t("Package"),
  generator: t("Generator"),
  exception: t("Exception"),
  domain: t("Domain"),
  keyword: t("Keyword"),
  type: t("Type"),
  variable: t("Built-in"),
};

const relationBoost = {
  table: 0,
  view: 0,
  // Rarely what is meant, and there are many of them
  system: -20,
};

const word = (w: Word, rest: Partial<Completion>): Completion => ({ label: w.name, apply: w.sql, ...rest });

function names(catalog: CompletionCatalog): Names {
  const relations = new Map(catalog.relations.map((r) => [r.name, r]));
  const top = [
    ...catalog.relations.map((r) => word(r, { type: r.kind, boost: relationBoost[r.kind] })),
    ...catalog.others.map((o) => word(o, { type: o.kind satisfies ObjectKind, boost: -1 })),
  ];
  return { relations, top, columns: new Map(), references: catalog.references };
}

function namesOf(registration: string): Promise<Names | null> {
  let pending = loaded.get(registration);
  if (!pending) {
    // A catalog that cannot be read completes nothing; the editor still works
    pending = api.completionCatalog(registration).then(names, () => null);
    loaded.set(registration, pending);
    void pending.then((n) => {
      if (!n && loaded.get(registration) === pending) loaded.delete(registration);
    });
  }
  return pending;
}

/** Reads a registration's names before the first key asks for them */
export function prepareCompletion(registration: string): void {
  if (useStore.getState().servers[registration]) void namesOf(registration);
}

function columnsOf(all: Names, relation: CatalogRelation, boost = 0): Completion[] {
  let columns = all.columns.get(relation);
  if (!columns) {
    columns = relation.columns.map((c) => word(c, { type: "column", detail: `${c.type} · ${relation.name}` }));
    all.columns.set(relation, columns);
  }
  return boost ? columns.map((c) => ({ ...c, boost })) : columns;
}

/** How far around the cursor a statement is looked for */
const REACH = 20_000;
const WORD = /^[\w$]*$/;
const QUOTED = /^"[^"]*"?$/;

/** What was begun with a quote is matched and written with its quotes */
function inQuotes(option: Completion): Completion {
  const name = `"${option.label.replaceAll('"', '""')}"`;
  return { ...option, label: name, apply: name };
}

async function complete(context: CompletionContext, registration: string | undefined): Promise<CompletionResult | null> {
  if (!registration || !useStore.getState().servers[registration]) return null;
  const inside = syntaxTree(context.state).resolveInner(context.pos, -1).name;
  if (inside === "String" || inside === "LineComment" || inside === "BlockComment") return null;

  const typed = context.matchBefore(/[\w$]*/);
  let from = typed ? typed.from : context.pos;
  let to: number | undefined;
  // A quoted name being typed is replaced whole, quotes and all
  const doc = context.state.doc;
  const quoted = from > 0 && doc.sliceString(from - 1, from) === '"';
  if (quoted) {
    from -= 1;
    if (doc.sliceString(context.pos, context.pos + 1) === '"') to = context.pos + 1;
  }

  const line = doc.lineAt(from);
  const parent = qualifier(doc.sliceString(line.from, from));
  if (!parent && from === context.pos && !context.explicit) return null;

  const all = await namesOf(registration);
  if (!all || context.aborted) return null;

  const start = Math.max(0, context.pos - REACH);
  const inReach = sources(doc.sliceString(start, Math.min(doc.length, context.pos + REACH)), context.pos - start);
  const result = (options: Completion[]): CompletionResult =>
    quoted ? { from, to, options: options.map(inQuotes), validFor: QUOTED } : { from, options, validFor: WORD };

  if (parent) {
    // An alias hides a relation of the same name
    const aliased = inReach.find((s) => s.alias === parent);
    const relation = all.relations.get(aliased ? aliased.relation : parent);
    return relation ? result(columnsOf(all, relation)) : null;
  }

  const here: Completion[] = [];
  const seen = new Set<CatalogRelation>();
  for (const source of inReach) {
    const relation = all.relations.get(source.relation);
    if (!relation) continue;
    if (source.alias) here.push({ label: source.alias, apply: source.written, type: "alias", detail: relation.name, boost: 10 });
    if (!seen.has(relation)) {
      seen.add(relation);
      here.push(...columnsOf(all, relation, 5));
    }
  }
  // After JOIN, what the foreign keys of the statement's tables lead to
  // comes first, written with its alias and its condition
  if (!quoted && /\bjoin\s+$/i.test(doc.sliceString(Math.max(start, from - 200), from))) {
    const others = sources(doc.sliceString(start, from) + doc.sliceString(context.pos, Math.min(doc.length, context.pos + REACH)), from - start);
    here.unshift(...joins(all, others));
  }
  return result(here.length ? [...here, ...all.top] : all.top);
}

/** The joins the foreign keys offer, each as the text that writes it */
function joins(all: Names, inStatement: ReturnType<typeof sources>): Completion[] {
  const options: Completion[] = [];
  const seen = new Set<string>();
  for (const join of joinsFor(inStatement, all.references)) {
    const relation = all.relations.get(join.relation);
    const other = all.relations.get(join.with.relation);
    if (!relation || !other) continue;
    const sql = (r: CatalogRelation, column: string) => r.columns.find((c) => c.name === column)?.sql ?? column;
    // Its keywords in the capitals the statement is written in
    const upper = /[A-Z]/.test(join.alias);
    const on = join.on
      .map(([own, theirs]) => `${join.alias}.${sql(relation, own)} = ${join.with.written}.${sql(other, theirs)}`)
      .join(upper ? " AND " : " and ");
    const text = `${relation.sql} ${join.alias} ${upper ? "ON" : "on"} ${on}`;
    if (seen.has(text)) continue;
    seen.add(text);
    // Matched by the relation's name alone, and ahead of the bare name
    options.push({ label: relation.name, displayLabel: text, apply: text, type: "join", boost: 99 });
  }
  return options;
}

/** The list as this editor shows it: what each name is in a first column,
 *  then the name, then what is known of it */
export const completionList: Extension = autocompletion({
  icons: false,
  addToOptions: [
    {
      position: 20,
      render(completion) {
        const kind = document.createElement("span");
        kind.className = `cm-completionKind cm-completionKind-${completion.type ?? "none"}`;
        kind.textContent = KINDS[completion.type ?? ""] ?? "";
        return kind;
      },
    },
  ],
});

/** Completes the names of the database `registration` says the editor is on */
export function catalogCompletion(registration: () => string | undefined): Extension {
  return firebirdDialect.language.data.of({
    autocomplete: (context: CompletionContext) => complete(context, registration()),
  });
}
