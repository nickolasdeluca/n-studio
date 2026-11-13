// Completion of a database's own names in SQL: relations and what else a
// statement names, a relation's columns after its name or its alias and a
// dot, the columns of what the statement already reads from, and after
// JOIN the joins its foreign keys offer, written whole with an alias.
// Keywords are the language's own source (firebird.ts).
import {
  autocompletion,
  completionStatus,
  insertCompletionText,
  pickedCompletion,
  startCompletion,
  type Completion,
  type CompletionContext,
  type CompletionResult,
} from "@codemirror/autocomplete";
import { syntaxTree } from "@codemirror/language";
import { EditorState, type Extension, type StateEffect, type Transaction, type TransactionSpec } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { api, type CatalogRelation, type CompletionCatalog, type DiagramReference, type ObjectKind, type Word } from "./api";
import { firebirdDialect } from "./firebird";
import { t } from "./i18n";
import { joinsFor, qualifier, sources } from "./sqlScope";
import { useStore } from "./store";

interface Names {
  relations: Map<string, CatalogRelation>;
  /** Everything that completes where no relation is named before a dot */
  top: Completion[];
  /** A relation's columns as they complete: as they are matched against
   *  what is typed, and as they are listed before anything is */
  columns: Map<CatalogRelation, { typed: Completion[]; listed: Completion[] }>;
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

/**
 * Writes a completion over the whole name the cursor is in, and not only
 * the part before it: picked in the middle of a name, it replaces the name.
 */
const whole = (text: string): Completion["apply"] => (view, completion, from, to) => {
  const rest = /^[\w$]*/.exec(view.state.sliceDoc(to, Math.min(view.state.doc.length, to + 256)))?.[0].length ?? 0;
  view.dispatch({ ...insertCompletionText(view.state, text, from, to + rest), annotations: pickedCompletion.of(completion) });
};

const word = (w: Word, rest: Partial<Completion>): Completion => ({ label: w.name, apply: whole(w.sql), ...rest });

/** What a list of columns starts with before anything is typed: the
 *  primary key, then the unique ones. Small, so that it only breaks ties. */
const KEY_BOOST = { primary: 2, unique: 1 };
const KEY_NAMES = { primary: "PK", unique: "UK" };

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

/**
 * A relation's columns. `keysFirst` is how they are listed before anything
 * is typed: the keys ahead. Typed against, the nearest match leads and a
 * key is a column like the others.
 */
function columnsOf(all: Names, relation: CatalogRelation, keysFirst: boolean, boost = 0): Completion[] {
  let columns = all.columns.get(relation);
  if (!columns) {
    const typed = relation.columns.map((c) =>
      word(c, { type: "column", detail: `${c.key ? `${KEY_NAMES[c.key]} · ` : ""}${c.type} · ${relation.name}` }),
    );
    const listed = typed.map((option, i) => {
      const key = relation.columns[i].key;
      return key ? { ...option, boost: KEY_BOOST[key] } : option;
    });
    columns = { typed, listed };
    all.columns.set(relation, columns);
  }
  const options = keysFirst ? columns.listed : columns.typed;
  return boost ? options.map((c) => ({ ...c, boost: boost + (c.boost ?? 0) })) : options;
}

/** How far around the cursor a statement is looked for */
const REACH = 20_000;
// A list made before anything was typed is made again at the first letter,
// and one made for some letters when the last of them is deleted: the two
// are not in the same order (`columnsOf`)
const NOTHING = /^$/;
const WORD = /^[\w$]+$/;
const QUOTED = /^"[^"]*"?$/;

/** What was begun with a quote is matched and written with its quotes */
function inQuotes(option: Completion): Completion {
  const name = `"${option.label.replaceAll('"', '""')}"`;
  return { ...option, label: name, apply: whole(name) };
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
  const bare = from === context.pos;
  const result = (options: Completion[]): CompletionResult =>
    quoted ? { from, to, options: options.map(inQuotes), validFor: QUOTED } : { from, options, validFor: bare ? NOTHING : WORD };

  if (parent) {
    // An alias hides a relation of the same name
    const aliased = inReach.find((s) => s.alias === parent);
    const relation = all.relations.get(aliased ? aliased.relation : parent);
    return relation ? result(columnsOf(all, relation, bare)) : null;
  }

  const here: Completion[] = [];
  const seen = new Set<CatalogRelation>();
  for (const source of inReach) {
    const relation = all.relations.get(source.relation);
    if (!relation) continue;
    if (source.alias) here.push({ label: source.alias, apply: whole(source.written), type: "alias", detail: relation.name, boost: 10 });
    if (!seen.has(relation)) {
      seen.add(relation);
      here.push(...columnsOf(all, relation, bare, 5));
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
    options.push({ label: relation.name, displayLabel: text, apply: whole(text), type: "join", boost: 99 });
  }
  return options;
}

/** The cursor went along a name, without typing, while a list was open */
function alongName(tr: Transaction): boolean {
  if (tr.docChanged || !tr.selection || !tr.isUserEvent("select") || completionStatus(tr.startState) !== "active") return false;
  const from = tr.startState.selection.main;
  const to = tr.newSelection.main;
  if (!from.empty || !to.empty || from.head === to.head) return false;
  // Still in the name: past a dot or a space, the list was for something else
  return /^[\w$]+$/.test(tr.newDoc.sliceString(Math.min(from.head, to.head), Math.max(from.head, to.head)));
}

/** The effect `startCompletion` dispatches, which the library keeps to
 *  itself: taken from a dispatch it is given, once */
let startEffect: StateEffect<unknown> | null | undefined;

function startEffectOf(state: EditorState): StateEffect<unknown> | null {
  if (startEffect === undefined) {
    startEffect = null;
    const dispatch = (spec: TransactionSpec) => {
      const effects = spec.effects;
      startEffect = (Array.isArray(effects) ? effects[0] : effects) ?? null;
    };
    startCompletion({ state, dispatch } as unknown as EditorView);
  }
  return startEffect;
}

/**
 * An open list follows the cursor along a name: each letter the cursor
 * passes going right narrows the list to what starts the name up to there,
 * and going left widens it again, as IBExpert's does. On its own a list
 * closes when the cursor moves without typing.
 *
 * The list is asked for again in the transaction that moves the cursor, so
 * it stays on the screen while the new one is made. Asked for afterwards,
 * it would close and open at every key, which is seen as a flicker; that
 * is still what happens should the library stop handing its effect over.
 */
const followCursor: Extension = [
  EditorState.transactionExtender.of((tr) => {
    if (!alongName(tr)) return null;
    const effect = startEffectOf(tr.startState);
    return effect ? { effects: effect } : null;
  }),
  EditorView.updateListener.of((update) => {
    if (!update.selectionSet || completionStatus(update.state) !== null) return;
    if (update.transactions.some(alongName)) startCompletion(update.view);
  }),
];

/** The list as this editor shows it: what each name is in a first column,
 *  then the name, then what is known of it */
export const completionList: Extension = [followCursor, autocompletion({
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
})];

/** Completes the names of the database `registration` says the editor is on */
export function catalogCompletion(registration: () => string | undefined): Extension {
  return firebirdDialect.language.data.of({
    autocomplete: (context: CompletionContext) => complete(context, registration()),
  });
}
