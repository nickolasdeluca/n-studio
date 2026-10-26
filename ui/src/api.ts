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

export interface StatementRange {
  from: number;
  to: number;
  /** Past the terminator */
  end: number;
  kind: { type: "sql" } | { type: "set_term"; terminator: string } | { type: "isql" };
  unterminatedLiteral: boolean;
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

  editorBegin: (id: string, options: TxOptions) => invoke<number>("editor_begin", { id, options }),
  editorEnd: (id: string, tx: number, how: TxEnd) => invoke<void>("editor_end", { id, tx, how }),
  editorCancel: (id: string) => invoke<boolean>("editor_cancel", { id }),
  editorExecute: (id: string, tx: number, sql: string, options: { plan: boolean; performance: boolean }, first: number) =>
    invoke<Executed>("editor_execute", { id, tx, sql, options, first }),
  resultFetch: (result: number, max: number) => invoke<ResultView>("result_fetch", { result, max }),
  resultRows: (result: number, from: number, count: number) => invoke<Cell[][]>("result_rows", { result, from, count }),
  resultClose: (result: number) => invoke<void>("result_close", { result }),
  sqlStatements: (text: string) => invoke<StatementRange[]>("sql_statements", { text }),
};

export function serverSummary(s: ServerInfo): string {
  return `Firebird ${s.engine_version} · ODS ${s.ods_major}.${s.ods_minor} · dialect ${s.dialect} · ${s.charset}`;
}
