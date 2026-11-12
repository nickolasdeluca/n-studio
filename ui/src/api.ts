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
  /** A non-text blob, as a hex dump */
  hex: string | null;
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

export interface GridRow {
  cells: Cell[];
  /** The object the row stands for, which opening the row opens */
  open: ObjectRef | null;
}

export interface GridSection {
  /** `null` for a tab's only grid */
  title: string | null;
  columns: GridColumn[];
  rows: GridRow[];
}

export type TabContent =
  | { shape: "grid"; sections: GridSection[] }
  | { shape: "sql"; text: string }
  | { shape: "text"; text: string }
  /** A query for the viewer to run and page through */
  | { shape: "data"; sql: string };

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
  objectTabs: (kind: ObjectKind) => invoke<TabInfo[]>("object_tabs", { kind }),
  objectTab: (id: string, kind: ObjectKind, name: string, tab: string) =>
    invoke<TabContent>("object_tab", { id, kind, name, tab }),
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
  resultClose: (result: number) => invoke<void>("result_close", { result }),
  resultSort: (result: number, sort: Sort | null) => invoke<ResultView>("result_sort", { result, sort }),
  resultExport: (result: number, path: string, options: ExportOptions) =>
    invoke<ExportSummary>("result_export", { result, path, options }),
  scriptRun: (id: string, run: number, tx: number | null, txOptions: TxOptions, text: string, options: ScriptOptions) =>
    invoke<ScriptEnd>("script_run", { id, run, tx, txOptions, text, options }),
  scriptCancel: (id: string, run: number) => invoke<boolean>("script_cancel", { id, run }),
  sqlStatements: (text: string) => invoke<StatementRange[]>("sql_statements", { text }),
  statementParameters: (sql: string) => invoke<Parameters>("statement_parameters", { sql }),
};

export function serverSummary(s: ServerInfo): string {
  return `Firebird ${s.engine_version} · ODS ${s.ods_major}.${s.ods_minor} · dialect ${s.dialect} · ${s.charset}`;
}
