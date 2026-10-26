// Firebird SQL for CodeMirror: the keywords of Firebird 5 (reserved and not),
// its types and built-in functions, PSQL included, and the editor's look in
// the app's tokens.
import { HighlightStyle, syntaxHighlighting } from "@codemirror/language";
import { SQLDialect, sql } from "@codemirror/lang-sql";
import { EditorView } from "@codemirror/view";
import { tags } from "@lezer/highlight";

const keywords = `
absolute action active add admin after all alter always and any as asc ascending at auto autonomous
before begin between bit_length block break by
call caller cascade case cast character_length char_length check close coalesce collate collation column
comment commit committed computed conditional connect constraint containing continue create cross current
current_connection current_date current_role current_time current_timestamp current_transaction
current_user cursor
data database day deallocate debug declare default deferred delete deleting deterministic desc descending
descriptor disconnect distinct do domain drop
else end entry_point escape exception execute exists exit external extract
fetch file filter first first_value for foreign free_it from full function
gdscode generated generator global grant granted grantor group
having hour
identity if ignore immediate in inactive increment index inner input_type insert inserting into is
isolation
join
key
lag last last_value lateral lead leading leave left length level like limbo linger local localtime
localtimestamp lock lower
manual matched matching max merge message middlename min minute module_name month
names national natural no not nulls null nullif
octet_length of offset on only open option or order os_name outer output_type over overflow overlay
package page page_size pages parameter partition password percent_rank plan position post_event precision
primary privileges procedure protected
range rank read recreate recursive references release replace requests reserv reserving restart restrict
retain returning returns revoke right role rollback row_count row_number rows
savepoint scalar_array schema second segment select sensitive sequence set shadow shared similar
singular size skip snapshot some sort sql sqlcode sqlstate stability start starting statement
statistics sub_type substring suspend
table temporary then ties time timeout to trailing transaction trigger trim truncate type
uncommitted union unique update updating upper user using
value values variable varying view
wait weekday when where while window with without work write
year yearday
`;

const types = `
bigint binary blob boolean char character date decfloat decimal double float int int128 integer nchar
numeric real smallint time timestamp varbinary varchar zone
`;

const builtins = `
abs acos acosh ascii_char ascii_val asin asinh atan atan2 atanh avg base64_decode base64_encode bin_and
bin_not bin_or bin_shl bin_shr bin_xor bit_length blob_append ceil ceiling char_to_uuid compare_decfloat
corr cos cosh cot count covar_pop covar_samp crypt_hash cume_dist dateadd datediff decode decrypt
encrypt exp first_day floor gen_id gen_uuid hash hex_decode hex_encode last_day list ln log log10 lpad
make_dbkey maxvalue minvalue mod normalize_decfloat nth_value ntile overlay pi position power quantize
rand rdb$get_context rdb$get_transaction_cn rdb$role_in_use rdb$set_context regr_avgx regr_avgy
regr_count regr_intercept regr_r2 regr_slope regr_sxx regr_sxy regr_syy replace reverse right round
rpad rsa_decrypt rsa_encrypt rsa_private rsa_public rsa_sign_hash rsa_verify_hash sign sin sinh sqrt
stddev_pop stddev_samp sum tan tanh totalorder trunc unicode_char unicode_val uuid_to_char var_pop
var_samp
`;

const words = (s: string) => s.trim().split(/\s+/).join(" ");

export const firebirdDialect = SQLDialect.define({
  keywords: words(keywords),
  types: words(types),
  builtin: words(builtins),
  // Double quotes name identifiers (dialect 3)
  identifierQuotes: '"',
  operatorChars: "*+-%<>!=&|^~/:",
  // :name in PSQL and parameters
  specialVar: ":",
  hashComments: false,
  slashComments: false,
  spaceAfterDashes: false,
  doubleDollarQuotedStrings: false,
  // _UTF8 'text' introducers
  charSetCasts: true,
  // q'{…}' strings (Firebird 3+)
  plsqlQuotingMechanism: true,
});

export const firebirdSql = sql({ dialect: firebirdDialect, upperCaseKeywords: true });

const highlight = HighlightStyle.define([
  { tag: tags.keyword, color: "var(--sql-keyword)", fontWeight: "600" },
  { tag: [tags.typeName, tags.standard(tags.name)], color: "var(--sql-type)" },
  { tag: [tags.string, tags.special(tags.string)], color: "var(--sql-string)" },
  { tag: [tags.number, tags.bool, tags.null], color: "var(--sql-number)" },
  { tag: [tags.lineComment, tags.blockComment], color: "var(--sql-comment)", fontStyle: "italic" },
  { tag: tags.special(tags.name), color: "var(--sql-param)" },
  { tag: tags.operator, color: "var(--muted)" },
]);

export const editorLook = [
  syntaxHighlighting(highlight),
  EditorView.theme({
    "&": { height: "100%", fontSize: "13px", backgroundColor: "var(--panel)", color: "var(--text)" },
    ".cm-scroller": {
      fontFamily: 'ui-monospace, "SF Mono", "Cascadia Mono", Menlo, Consolas, monospace',
      lineHeight: "1.5",
    },
    ".cm-content": { caretColor: "var(--text)", userSelect: "text" },
    "&.cm-focused": { outline: "none" },
    ".cm-cursor": { borderLeftColor: "var(--text)" },
    ".cm-gutters": { backgroundColor: "var(--panel-2)", color: "var(--faint)", border: "none", borderRight: "1px solid var(--border)" },
    ".cm-activeLine": { backgroundColor: "color-mix(in srgb, var(--hover) 60%, transparent)" },
    ".cm-activeLineGutter": { backgroundColor: "var(--hover)", color: "var(--muted)" },
    "&.cm-focused .cm-selectionBackground, .cm-selectionBackground, ::selection": {
      backgroundColor: "var(--selected) !important",
    },
    ".cm-matchingBracket": { backgroundColor: "var(--selected)", outline: "1px solid var(--accent)" },
    ".cm-tooltip": { backgroundColor: "var(--panel)", border: "1px solid var(--border)", boxShadow: "var(--shadow)" },
    ".cm-tooltip-autocomplete > ul > li[aria-selected]": { backgroundColor: "var(--selected)", color: "var(--text)" },
    ".cm-panels": { backgroundColor: "var(--panel-2)", color: "var(--text)" },
    ".cm-panels.cm-panels-top": { borderBottom: "1px solid var(--border)" },
    ".cm-searchMatch": { backgroundColor: "color-mix(in srgb, var(--accent) 25%, transparent)" },
    ".cm-error-statement": { backgroundColor: "color-mix(in srgb, var(--danger) 14%, transparent)" },
  }),
];
