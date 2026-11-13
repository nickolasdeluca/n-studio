// The backend's commands, typed. Rust owns connections and metadata; this is
// the only place the UI talks to it.
import { invoke } from "@tauri-apps/api/core";

export type Dialect = "1" | "3";
export type Auth = "srp" | "legacy";

export interface FirebirdRegistration {
  engine: "firebird";
  /** Empty until saved */
  id: string;
  name: string;
  savePassword: boolean;
  host: string;
  port: number;
  database: string;
  user: string;
  role: string;
  charset: string;
  dialect: Dialect;
  auth: Auth;
}

/** One per engine; Firebird is the only one so far */
export type Registration = FirebirdRegistration;

export interface ServerInfo {
  engine_version: string;
  major: number;
  minor: number;
  ods_major: number;
  ods_minor: number;
  page_size: number;
  dialect: number;
  charset: string;
}

export type RegistrationView = Registration & { server: ServerInfo | null };

/** An object kind, as the backend names it: "table", "view", … */
export type ObjectKind = string;

export interface TreeKind {
  kind: ObjectKind;
  label: string;
  /** One of the kind: "Table" */
  singular: string;
  count: number;
}

export interface DbObject {
  name: string;
  parent: string | null;
  description: string | null;
}

export interface AppError {
  kind:
    | "server"
    | "network"
    | "connection"
    | "password_required"
    | "not_connected"
    | "not_found"
    | "invalid"
    | "storage"
    | "keychain";
  message?: string;
  sqlcode?: number;
  registration?: string;
}

export function isAppError(e: unknown): e is AppError {
  return typeof e === "object" && e !== null && "kind" in e;
}

export function errorText(e: unknown): string {
  if (isAppError(e)) {
    if (e.kind === "password_required") return `Enter the password for ${e.registration}.`;
    return e.message ?? e.kind;
  }
  return e instanceof Error ? e.message : String(e);
}

// The SQL editor

export type Isolation = "read_committed" | "read_committed_no_rec_version" | "snapshot" | "snapshot_table_stability";
export type LockWait = { mode: "wait" } | { mode: "no_wait" } | { mode: "timeout"; seconds: number };

export interface TxOptions {
  isolation: Isolation;
  readOnly: boolean;
  lockWait: LockWait;
}

export type TxEnd = "commit" | "commit_retaining" | "rollback" | "rollback_retaining";

export type StatementType =
  | "select"
  | "select_for_update"
  | "insert"
  | "update"
  | "delete"
  | "ddl"
  | "exec_procedure"
  | "set_generator"
  | "savepoint"
  | "commit"
  | "rollback"
  | "start_transaction"
  | "other";

export type ColumnKind =
  | "integer"
  | "decimal"
  | "float"
  | "text"
  | "date"
  | "time"
  | "timestamp"
  | "boolean"
  | "text_blob"
  | "blob"
  | "other";

export interface ColumnInfo {
  name: string;
  field: string;
  relation: string;
  typeName: string;
  kind: ColumnKind;
  nullable: boolean;
}

export interface Records {
  selected: number;
  inserted: number;
  updated: number;
  deleted: number;
}

export interface TablePerformance {
  table: string;
  indexedReads: number;
  nonIndexedReads: number;
  inserts: number;
  updates: number;
  deletes: number;
  backouts: number;
  purges: number;
  expunges: number;
}

export interface Performance {
  reads: number;
  writes: number;
  fetches: number;
  marks: number;
  currentMemory: number;
  maxMemory: number;
  buffers: number;
  pageSize: number;
  tables: TablePerformance[];
}

/** A cell as the grid shows it; null is SQL NULL */
export type Cell = string | null;

/** A select's rows, held by the backend */
export interface ResultView {
  id: number;
  /** Fetched so far */
  rows: number;
  eof: boolean;
  /** The transaction ended before the rows ran out */
  endedEarly: boolean;
  fetchMs: number;
  performance: Performance | null;
  /** How the rows are sorted; `null` is fetch order */
  sort: Sort | null;
}

export interface Sort {
  column: number;
  descending: boolean;
}

export interface ExportOptions {
  delimiter: string;
  header: boolean;
  bom: boolean;
}

export interface ExportSummary {
  rows: number;
  /** The transaction had ended: blob columns hold their markers */
  blobsUnread: boolean;
}

export interface Executed {
  statementType: StatementType;
  columns: ColumnInfo[];
  result: ResultView | null;
  /** An EXECUTE PROCEDURE's output row */
  rows: Cell[][];
  records: Records;
  plan: string | null;
  explainedPlan: string | null;
  prepareMs: number;
  executeMs: number;
  ended: TxEnd | null;
  performance: Performance | null;
}

/** A blob cell's content, read on demand */
export interface BlobView {
  text: string | null;
  /** Any other blob's bytes, in base64 */
  data: string | null;
  size: number;
  complete: boolean;
}

export interface StatementRange {
  from: number;
  to: number;
  /** Past the terminator */
  end: number;
  kind: { type: "sql" } | { type: "set_term"; terminator: string } | { type: "isql" };
  unterminatedLiteral: boolean;
}

export interface ScriptOptions {
  /** Stop at the first statement that fails */
  stopOnError: boolean;
  /** Commit after each DDL statement, until the script says otherwise */
  autoDdl: boolean;
  /** All or nothing, in a transaction of the script's own */
  atomic: boolean;
}

/** A statement of a script that did not run to its end */
export interface ScriptProblem {
  index: number;
  from: number;
  to: number;
  /** `null` for an isql command, passed over */
  error: string | null;
  sqlcode: number | null;
}

export interface DiagramColumn {
  name: string;
  typeName: string;
  domain: string | null;
  primaryKey: boolean;
  foreignKey: boolean;
  required: boolean;
}

export interface DiagramTable {
  name: string;
  columns: DiagramColumn[];
  /** Related tables the diagram does not show */
  hidden: string[];
}

/** A foreign key: `from` holds it, `to` has the key it points at */
export interface DiagramReference {
  name: string;
  from: string;
  fromColumns: string[];
  to: string;
  toColumns: string[];
}

/** A statement an editor ran */
export interface HistoryEntry {
  id: number;
  /** Milliseconds since 1970, when it last ran */
  at: number;
  sql: string;
  elapsedMs: number;
  /** What the server said, when it failed */
  error: string | null;
  /** Times in a row it was run */
  runs: number;
}

/** How a script's file holds its text */
export interface FileFormat {
  encoding: "utf8" | "utf8_bom" | "utf16_le" | "utf16_be" | "win1252";
  /** Lines end in CR LF; the editor's text has LF alone */
  crlf: boolean;
}

export interface ScriptFile {
  text: string;
  format: FileFormat;
}

/** An editor as it is kept between runs of the app */
export interface SessionTab {
  registration: string;
  title: string;
  file: string | null;
  format: FileFormat | null;
  /** Null when it is the file's, unchanged */
  text: string | null;
}

export interface Session {
  tabs: SessionTab[];
  /** Which of them was in front */
  active: number | null;
}

/** A name, and how SQL writes it in its database */
export interface Word {
  name: string;
  sql: string;
}

export interface CatalogRelation extends Word {
  kind: "table" | "view" | "system";
  /** `type` as it would be declared: `VARCHAR(60)` */
  columns: (Word & { type: string })[];
}

/** Every name an SQL editor completes */
export interface CompletionCatalog {
  relations: CatalogRelation[];
  others: (Word & { kind: ObjectKind })[];
  /** Every foreign key: what a join is written from */
  references: DiagramReference[];
}

export interface Diagram {
  tables: DiagramTable[];
  references: DiagramReference[];
  /** Every table of the database */
  all: string[];
}

export type JoinKind = "inner" | "left" | "right" | "full";
export type Aggregate = "count" | "sum" | "avg" | "min" | "max";

export interface QueryTable {
  name: string;
  /** What the statement calls it: its name, unless it is there twice */
  alias: string;
}

export interface QueryJoin {
  kind: JoinKind;
  /** Aliases */
  left: string;
  right: string;
  /** A column of `left` and the column of `right` it equals */
  on: [string, string][];
}

export interface QueryColumn {
  table: string;
  column: string;
  alias: string | null;
  aggregate: Aggregate | null;
  sort: "asc" | "desc" | null;
  /** As it is written after the column: `> 10`, `IS NULL` */
  filter: string | null;
  output: boolean;
}

export interface QuerySpec {
  tables: QueryTable[];
  joins: QueryJoin[];
  columns: QueryColumn[];
  distinct: boolean;
  first: number | null;
}

export interface DebugVar {
  name: string;
  kind: "input" | "output" | "local";
  typeName: string;
  /** `null` is NULL */
  value: string | null;
}

export interface DebugNote {
  line: number;
  what: string;
}

/** A routine opened to debug */
export interface DebugTarget {
  source: string;
  inputs: DebugVar[];
  outputs: string[];
  /** The statements the debugger will refuse to run */
  notes: DebugNote[];
}

/** Where a debugged run is. `finished` waits for its transaction to be
 *  ended; `stopped` and `failed` have rolled it back. */
export interface DebugState {
  run: number;
  state: "paused" | "finished" | "ended" | "stopped" | "failed";
  line: number | null;
  endLine: number | null;
  variables: DebugVar[];
  /** The rows suspended since the last event */
  rows: (string | null)[][];
  rowCount: number;
  notes: DebugNote[];
  error: string | null;
}

export type DebugCommand = "step" | "continue" | "stop" | "commit" | "rollback" | "";

export type DiffKind = "only_source" | "only_target" | "different";

export interface DiffRow {
  kind: DiffKind;
  key: (string | null)[];
  changes: { column: string; source: string | null; target: string | null }[];
}

/** Two tables' rows compared: counts of them all, and as many differences
 *  as were kept */
export interface Comparison {
  key: string[];
  columns: string[];
  notes: string[];
  sourceRows: number;
  targetRows: number;
  same: number;
  onlySource: number;
  onlyTarget: number;
  different: number;
  rows: DiffRow[];
  truncated: boolean;
  deletes: string;
  updates: string;
  inserts: string;
  cancelled: boolean;
}

/** The `compare-progress` event */
export interface CompareProgress {
  run: number;
  phase: string;
  rows: number;
}

export type AdminView = "monitor" | "users";

/** The `service-output` event */
export interface ServiceOutput {
  run: number;
  lines: string[];
}

export interface ServiceResult {
  end: "finished" | "cancelled";
  lines: string[];
}

/** The `script-progress` event */
export interface ScriptProgress {
  run: number;
  at: number;
  statements: number;
  failed: number;
  problems: ScriptProblem[];
}

export interface ScriptEnd {
  statements: number;
  done: number;
  failed: number;
  skipped: number;
  /** DDL statements among those done */
  ddl: number;
  cancelled: boolean;
  /** The transaction left active */
  tx: number | null;
  /** An atomic script's end */
  ended: TxEnd | null;
  elapsedMs: number;
  /** Those no progress event carried */
  problems: ScriptProblem[];
}

/** A statement with its `:name`s turned into `?`, and each `?`'s name */
export interface Parameters {
  sql: string;
  /** `null` for a `?` */
  names: (string | null)[];
}

// The object viewers

export type TabShape = "grid" | "sql" | "text" | "data";

/** One tab of a viewer, before its content is asked for */
export interface TabInfo {
  id: string;
  label: string;
  shape: TabShape;
}

export type CellKind = "text" | "number" | "flag" | "code";

export interface GridColumn {
  label: string;
  kind: CellKind;
}

export interface ObjectRef {
  kind: ObjectKind;
  name: string;
}

/** Something to do to an object, or to a part of it */
export interface Action {
  id: string;
  label: string;
  /** It asks for values first */
  form: boolean;
  /** It destroys something */
  danger: boolean;
}

export type FieldKind = "text" | "password" | "code" | "number" | "check" | "select" | "combo";

export interface FormField {
  id: string;
  label: string;
  kind: FieldKind;
  /** What is there now; a check's is "true" or "false" */
  value: string;
  options: string[];
  required: boolean;
  hint: string | null;
}

/** What an action asks before it can be a script */
export interface Form {
  title: string;
  fields: FormField[];
}

export interface ActionScript {
  script: string;
  /** What the script is about: the object, and the part of it */
  title: string;
  /** The name the object goes by once the script ran, when it renames it */
  renamed: string | null;
}

export interface GridRow {
  cells: Cell[];
  /** The object the row stands for, which opening the row opens */
  open: ObjectRef | null;
  /** What the section's row actions act on */
  key: string | null;
}

export interface GridSection {
  /** `null` for a tab's only grid */
  title: string | null;
  columns: GridColumn[];
  rows: GridRow[];
  /** What can be done here */
  actions: Action[];
  /** What can be done to a row that has a key */
  row_actions: Action[];
}

export type TabContent =
  | { shape: "grid"; sections: GridSection[] }
  /** `editable`: the text can be changed and compiled back */
  | { shape: "sql"; text: string; editable: boolean }
  | { shape: "text"; text: string; editable: boolean }
  /** A query for the viewer to run and page through; `edit` when its rows
   *  can be changed */
  | { shape: "data"; sql: string; edit: RowEditing | null };

/** How a table's rows can be changed from the grid */
export interface RowEditing {
  /** The columns that tell a row from every other */
  key: string[];
  /** The columns a value can be typed into */
  editable: string[];
}

export interface Assign {
  column: string;
  /** `null` is NULL */
  value: Cell;
}

/** What was done to a grid's rows and is not in the table yet */
export interface RowChanges {
  updates: { key: Cell[]; set: Assign[] }[];
  deletes: Cell[][];
  inserts: Assign[][];
}

export const api = {
  charsets: () => invoke<string[]>("charsets"),
  listRegistrations: () => invoke<RegistrationView[]>("list_registrations"),
  saveRegistration: (registration: Registration, password: string | null) =>
    invoke<Registration>("save_registration", { registration, password }),
  deleteRegistration: (id: string) => invoke<void>("delete_registration", { id }),
  testConnection: (registration: Registration, password: string | null) =>
    invoke<ServerInfo>("test_connection", { registration, password }),
  connect: (id: string, password: string | null, remember: boolean) =>
    invoke<ServerInfo>("connect", { id, password, remember }),
  disconnect: (id: string) => invoke<void>("disconnect", { id }),
  explorerTree: (id: string) => invoke<TreeKind[]>("explorer_tree", { id }),
  objectList: (id: string, kind: ObjectKind) => invoke<DbObject[]>("object_list", { id, kind }),
  scriptRead: (path: string) => invoke<ScriptFile>("script_read", { path }),
  /** Answers the format written: UTF-8 where the one asked for lacks a character */
  scriptWrite: (path: string, text: string, format: FileFormat) => invoke<FileFormat>("script_write", { path, text, format }),
  /** False when the statement is not one that is kept */
  historyAdd: (id: string, sql: string, elapsedMs: number, error: string | null) =>
    invoke<boolean>("history_add", { id, sql, elapsedMs, error }),
  /** The latest first; those with `search` in them when it says anything */
  historyList: (id: string, search: string, limit: number) => invoke<HistoryEntry[]>("history_list", { id, search, limit }),
  historyClear: (id: string) => invoke<null>("history_clear", { id }),
  sessionLoad: () => invoke<Session>("session_load"),
  sessionSave: (session: Session) => invoke<null>("session_save", { session }),
  completionCatalog: (id: string) => invoke<CompletionCatalog>("completion_catalog", { id }),
  objectTabs: (kind: ObjectKind) => invoke<TabInfo[]>("object_tabs", { kind }),
  objectTab: (id: string, kind: ObjectKind, name: string, tab: string) =>
    invoke<TabContent>("object_tab", { id, kind, name, tab }),
  /** The script that makes an editable tab's new text the object's */
  objectTabScript: (id: string, kind: ObjectKind, name: string, tab: string, text: string) =>
    invoke<string>("object_tab_script", { id, kind, name, tab, text }),
  /** The form an action fills before it runs, with what is there now */
  objectActionForm: (id: string, kind: ObjectKind, name: string, action: string, key: string | null) =>
    invoke<Form>("object_action_form", { id, kind, name, action, key }),
  /** The script that carries an action out, with its form's values */
  objectActionScript: (
    id: string,
    kind: ObjectKind,
    name: string,
    action: string,
    key: string | null,
    values: Record<string, string>,
  ) => invoke<ActionScript>("object_action_script", { id, kind, name, action, key, values }),
  /** The script that makes a grid's changes the table's */
  dataChangesScript: (id: string, table: string, changes: RowChanges) =>
    invoke<string>("data_changes_script", { id, table, changes }),
  objectDropScript: (id: string, kind: ObjectKind, name: string) => invoke<string>("object_drop_script", { id, kind, name }),
  objectTemplate: (id: string, kind: ObjectKind) => invoke<string>("object_template", { id, kind }),
  extractMetadata: (id: string, objects: ObjectRef[]) => invoke<string>("extract_metadata", { id, objects }),

  editorBegin: (id: string, options: TxOptions) => invoke<number>("editor_begin", { id, options }),
  editorEnd: (id: string, tx: number, how: TxEnd) => invoke<void>("editor_end", { id, tx, how }),
  editorCancel: (id: string) => invoke<boolean>("editor_cancel", { id }),
  editorDescribe: (id: string, tx: number, sql: string) => invoke<ColumnInfo[]>("editor_describe", { id, tx, sql }),
  editorExecute: (
    id: string,
    tx: number,
    sql: string,
    params: (string | null)[],
    options: { plan: boolean; performance: boolean },
    first: number,
  ) => invoke<Executed>("editor_execute", { id, tx, sql, params, options, first }),
  resultFetch: (result: number, max: number) => invoke<ResultView>("result_fetch", { result, max }),
  resultRows: (result: number, from: number, count: number) => invoke<Cell[][]>("result_rows", { result, from, count }),
  resultBlob: (result: number, row: number, col: number, max: number) =>
    invoke<BlobView | null>("result_blob", { result, row, col, max }),
  /** Writes the cell's whole blob to a file; the bytes written come back */
  resultBlobSave: (result: number, row: number, col: number, path: string) =>
    invoke<number>("result_blob_save", { result, row, col, path }),
  resultClose: (result: number) => invoke<void>("result_close", { result }),
  resultSort: (result: number, sort: Sort | null) => invoke<ResultView>("result_sort", { result, sort }),
  resultExport: (result: number, path: string, options: ExportOptions) =>
    invoke<ExportSummary>("result_export", { result, path, options }),
  scriptRun: (id: string, run: number, tx: number | null, txOptions: TxOptions, text: string, options: ScriptOptions) =>
    invoke<ScriptEnd>("script_run", { id, run, tx, txOptions, text, options }),
  scriptCancel: (id: string, run: number) => invoke<boolean>("script_cancel", { id, run }),
  /** The grids of the activity monitor ("monitor", `idle` for the idle
   *  statements too) or of the server's users ("users") */
  adminView: (id: string, view: AdminView, idle: boolean) => invoke<GridSection[]>("admin_view", { id, view, idle }),
  /** What a tool asks before it runs; `key` is the row it acts on */
  adminForm: (id: string, tool: string, key: string | null) => invoke<Form>("admin_form", { id, tool, key }),
  /** The script that carries a tool out, to be shown and then run */
  adminScript: (id: string, tool: string, key: string | null, values: Record<string, string>) =>
    invoke<ActionScript>("admin_script", { id, tool, key, values }),
  /** Runs a service on the registration's server; its output comes as
   *  `service-output` events, and what no event carried with the answer */
  serviceRun: (id: string, run: number, tool: string, values: Record<string, string>) =>
    invoke<ServiceResult>("service_run", { id, run, tool, values }),
  serviceCancel: (run: number) => invoke<boolean>("service_cancel", { run }),
  /** The tables named and what is within `depth` references of them; the
   *  whole database's when none is named */
  diagram: (id: string, tables: string[], depth: number) => invoke<Diagram>("diagram", { id, tables, depth }),
  writeTextFile: (path: string, text: string) => invoke<null>("write_text_file", { path, text }),
  /** Compares a table of the source registration with one of the target's */
  dataCompare: (run: number, source: string, sourceTable: string, target: string, targetTable: string) =>
    invoke<Comparison>("data_compare", { run, source, sourceTable, target, targetTable }),
  dataCompareCancel: (run: number) => invoke<boolean>("data_compare_cancel", { run }),
  /** The tables and views named, the references between them, and the
   *  names of all the others */
  querySources: (id: string, names: string[]) => invoke<Diagram>("query_sources", { id, names }),
  /** The SELECT the parts make */
  queryBuild: (id: string, spec: QuerySpec) => invoke<string>("query_build", { id, spec }),
  debugOpen: (id: string, kind: ObjectKind, name: string) => invoke<DebugTarget>("debug_open", { id, kind, name }),
  /** Starts a run; what it does arrives as `debug-state` events */
  debugStart: (id: string, run: number, kind: ObjectKind, name: string, inputs: (string | null)[], breakpoints: number[], step: boolean) =>
    invoke<null>("debug_start", { id, run, kind, name, inputs, breakpoints, step }),
  debugCommand: (run: number, command: DebugCommand, breakpoints: number[] | null) =>
    invoke<boolean>("debug_command", { run, command, breakpoints }),
  sqlStatements: (text: string) => invoke<StatementRange[]>("sql_statements", { text }),
  statementParameters: (sql: string) => invoke<Parameters>("statement_parameters", { sql }),
};

export function serverSummary(s: ServerInfo): string {
  return `Firebird ${s.engine_version} · ODS ${s.ods_major}.${s.ods_minor} · dialect ${s.dialect} · ${s.charset}`;
}
