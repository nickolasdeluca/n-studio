// The UI in a browser, without the backend: `npm run dev`, then
// http://localhost:5173/preview.html. Every command is answered here with
// made-up data in the shape the backend gives (the docker fixture's
// objects), so a layout can be looked at, clicked through and screenshot
// where the app's own window cannot be driven. Not part of the build.
import { mockIPC, mockWindows } from "@tauri-apps/api/mocks";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import type {
  Cell,
  ColumnInfo,
  DbObject,
  Executed,
  GridColumn,
  GridRow,
  ObjectRef,
  RegistrationView,
  ResultView,
  ServerInfo,
  TabContent,
  TabInfo,
  TreeKind,
} from "./api";
import { Shell } from "./components/Shell";
import "./index.css";

const server: ServerInfo = {
  engine_version: "4.0.4",
  major: 4,
  minor: 0,
  ods_major: 13,
  ods_minor: 0,
  page_size: 8192,
  dialect: 3,
  charset: "UTF8",
};

const registrations: RegistrationView[] = [
  {
    engine: "firebird",
    id: "fixture",
    name: "Fixture",
    savePassword: true,
    host: "localhost",
    port: 3050,
    database: "/data/fixture.fdb",
    user: "SYSDBA",
    role: "",
    charset: "UTF8",
    dialect: "3",
    auth: "srp",
    server: null,
  },
];

const o = (name: string, parent: string | null = null, description: string | null = null): DbObject => ({ name, parent, description });
const bulk = Array.from({ length: 400 }, (_, i) => o(`TABELA_${String(i + 1).padStart(3, "0")}`));
const objects: Record<string, DbObject[]> = {
  domain: [o("D_FLAG"), o("D_ID"), o("D_MONEY")],
  table: [o("BULK_ROWS"), o("PEDIDO", null, "Orders"), o("PEDIDO_ITEM"), o("TIPOS"), ...bulk],
  view: [o("V_PEDIDO_TOTAL"), o("V_TIPOS")],
  procedure: [o("P_FIXTURE"), o("P_PEDIDO_TOTAL"), o("P_SEED_BULK")],
  function: [],
  package: [o("PKG_FIXTURE")],
  trigger: [o("TIPOS_BI", "TIPOS")],
  generator: [o("GEN_TIPOS_ID")],
  exception: [o("E_FIXTURE")],
  udf: [],
  role: [],
  index: [o("IDX_BULK_NOME", "BULK_ROWS"), o("IDX_ITEM_PRECO", "PEDIDO_ITEM"), o("PK_PEDIDO", "PEDIDO")],
};
const labels: Record<string, string> = {
  domain: "Domains",
  table: "Tables",
  view: "Views",
  procedure: "Procedures",
  function: "Functions",
  package: "Packages",
  trigger: "Triggers",
  generator: "Generators",
  exception: "Exceptions",
  udf: "UDFs",
  role: "Roles",
  index: "Indices",
};
const tree: TreeKind[] = Object.keys(labels).map((kind) => ({ kind, label: labels[kind], count: objects[kind].length }));

const tab = (id: string, label: string, shape: TabInfo["shape"]): TabInfo => ({ id, label, shape });
const FIELDS = tab("fields", "Fields", "grid");
const DEPENDENCIES = tab("dependencies", "Dependencies", "grid");
const TRIGGERS = tab("triggers", "Triggers", "grid");
const DATA = tab("data", "Data", "data");
const DESCRIPTION = tab("description", "Description", "text");
const DDL = tab("ddl", "DDL", "sql");
const PROPERTIES = tab("properties", "Properties", "grid");
function tabsOf(kind: string): TabInfo[] {
  switch (kind) {
    case "table":
      return [FIELDS, tab("constraints", "Constraints", "grid"), tab("indices", "Indices", "grid"), DEPENDENCIES, TRIGGERS, DATA, DESCRIPTION, DDL];
    case "view":
      return [FIELDS, DEPENDENCIES, TRIGGERS, DATA, DESCRIPTION, DDL];
    case "procedure":
    case "function":
      return [DDL, tab("parameters", "Parameters", "grid"), DEPENDENCIES, DESCRIPTION];
    case "package":
    case "trigger":
    case "udf":
      return [DDL, DEPENDENCIES, DESCRIPTION];
    case "role":
      return [PROPERTIES, DESCRIPTION, DDL];
    default:
      return [PROPERTIES, DEPENDENCIES, DESCRIPTION, DDL];
  }
}

const col = (label: string, kind: GridColumn["kind"] = "text"): GridColumn => ({ label, kind });
const row = (cells: Cell[], open: ObjectRef | null = null): GridRow => ({ cells, open });
const ref = (kind: string, name: string): ObjectRef => ({ kind, name });
const grid = (columns: GridColumn[], rows: GridRow[], title: string | null = null) => ({ title, columns, rows });

const tableDdl = `CREATE TABLE PEDIDO_ITEM (
    PEDIDO  D_ID,
    ITEM    SMALLINT NOT NULL,
    QTD     NUMERIC(9,3) DEFAULT 1 NOT NULL,
    PRECO   D_MONEY,
    TOTAL   COMPUTED BY (QTD * PRECO),
    PAGO    D_FLAG
);

ALTER TABLE PEDIDO_ITEM ADD CONSTRAINT PK_PEDIDO_ITEM PRIMARY KEY (PEDIDO, ITEM);
ALTER TABLE PEDIDO_ITEM ADD CONSTRAINT FK_ITEM_PEDIDO FOREIGN KEY (PEDIDO) REFERENCES PEDIDO (ID) ON DELETE CASCADE;
ALTER TABLE PEDIDO_ITEM ADD CONSTRAINT CK_ITEM_QTD CHECK (QTD > 0);

CREATE DESCENDING INDEX IDX_ITEM_PRECO ON PEDIDO_ITEM (PRECO);
`;
const procedureDdl = `SET TERM ^ ;

CREATE OR ALTER PROCEDURE P_PEDIDO_TOTAL (
    P_PEDIDO TYPE OF D_ID,
    P_MINIMO NUMERIC(18,4) = 0)
RETURNS (
    R_TOTAL TYPE OF COLUMN PEDIDO_ITEM.PRECO)
AS
BEGIN
  SELECT V.TOTAL FROM V_PEDIDO_TOTAL V WHERE V.PEDIDO = :P_PEDIDO INTO :R_TOTAL;
  IF (R_TOTAL < P_MINIMO) THEN
    EXCEPTION E_FIXTURE; -- 'fixture exception'
  SUSPEND;
END^

SET TERM ; ^
`;

function content(kind: string, name: string, id: string): TabContent {
  switch (id) {
    case "ddl":
      return { shape: "sql", text: kind === "table" ? tableDdl.replaceAll("PEDIDO_ITEM", name) : procedureDdl };
    case "description":
      return { shape: "text", text: name === "PEDIDO" ? "Orders\n\nOne row per order; its lines are in PEDIDO_ITEM." : "" };
    case "data":
      return { shape: "data", sql: `select * from ${name}` };
    case "fields": {
      const columns = [
        col("#", "number"),
        col("PK", "flag"),
        col("FK", "flag"),
        col("Name"),
        col("Type"),
        col("Domain"),
        col("Not null", "flag"),
        col("Default", "code"),
        col("Computed by", "code"),
        col("Character set"),
        col("Collation"),
        col("Description"),
      ];
      return {
        shape: "grid",
        sections: [
          grid(columns, [
            row(["1", "✓", "✓", "PEDIDO", "INTEGER", "D_ID", "✓", null, null, null, null, null], ref("domain", "D_ID")),
            row(["2", "✓", null, "ITEM", "SMALLINT", null, "✓", null, null, null, null, "Line number"]),
            row(["3", null, null, "QTD", "NUMERIC(9,3)", null, "✓", "DEFAULT 1", null, null, null, null]),
            row(["4", null, null, "PRECO", "NUMERIC(18,4)", "D_MONEY", null, "DEFAULT 0", null, null, null, null], ref("domain", "D_MONEY")),
            row(["5", null, null, "TOTAL", "NUMERIC(18,7)", null, null, null, "(QTD * PRECO)", null, null, null]),
            row(["6", null, null, "PAGO", "CHAR(1)", "D_FLAG", null, "DEFAULT 'N'", null, "UTF8", null, null], ref("domain", "D_FLAG")),
            row(["7", null, null, "OBS", "VARCHAR(200)", null, null, null, null, "WIN1252", "WIN_PTBR", "A note long enough that the column has to cut it somewhere, to see how the grid deals with it"]),
          ]),
        ],
      };
    }
    case "constraints":
      return {
        shape: "grid",
        sections: [
          grid(
            [col("Name"), col("Type"), col("On"), col("References"), col("On update"), col("On delete"), col("Index"), col("Check", "code")],
            [
              row(["PK_PEDIDO_ITEM", "Primary key", "PEDIDO, ITEM", null, null, null, "PK_PEDIDO_ITEM", null]),
              row(["FK_ITEM_PEDIDO", "Foreign key", "PEDIDO", "PEDIDO (ID)", "RESTRICT", "CASCADE", "FK_ITEM_PEDIDO", null], ref("table", "PEDIDO")),
              row(["CK_ITEM_QTD", "Check", null, null, null, null, null, "CHECK (QTD > 0)"]),
            ],
          ),
        ],
      };
    case "indices":
      return {
        shape: "grid",
        sections: [
          grid(
            [col("Name"), col("On"), col("Unique", "flag"), col("Descending", "flag"), col("Active", "flag"), col("Enforces"), col("Selectivity", "number"), col("Condition", "code"), col("Description")],
            [
              row(["FK_ITEM_PEDIDO", "PEDIDO", null, null, "✓", "Foreign key", "0.500000", null, null], ref("index", "FK_ITEM_PEDIDO")),
              row(["IDX_ITEM_PRECO", "PRECO", null, "✓", "✓", null, "0.250000", null, null], ref("index", "IDX_ITEM_PRECO")),
              row(["PK_PEDIDO_ITEM", "PEDIDO, ITEM", "✓", null, "✓", "Primary key", "2.500e-5", null, null], ref("index", "PK_PEDIDO_ITEM")),
            ],
          ),
        ],
      };
    case "triggers":
      return {
        shape: "grid",
        sections: [
          grid(
            [col("Name"), col("Fires"), col("Position", "number"), col("Active", "flag"), col("Description")],
            name === "TIPOS" ? [row(["TIPOS_BI", "BEFORE INSERT", "0", "✓", null], ref("trigger", "TIPOS_BI"))] : [],
          ),
        ],
      };
    case "dependencies": {
      const columns = [col("Type"), col("Name"), col("Part"), col("Columns")];
      return {
        shape: "grid",
        sections: [
          grid(
            columns,
            [
              row(["Domain", "D_FLAG", "PAGO", null], ref("domain", "D_FLAG")),
              row(["Domain", "D_ID", "PEDIDO", null], ref("domain", "D_ID")),
              row(["Foreign key", "PEDIDO", "FK_ITEM_PEDIDO", "ID"], ref("table", "PEDIDO")),
            ],
            "Uses",
          ),
          grid(columns, [row(["View", "V_PEDIDO_TOTAL", null, "PEDIDO, TOTAL"], ref("view", "V_PEDIDO_TOTAL"))], "Used by"),
        ],
      };
    }
    case "parameters": {
      const columns = [col("#", "number"), col("Name"), col("Type"), col("Not null", "flag"), col("Default", "code"), col("Character set"), col("Description")];
      return {
        shape: "grid",
        sections: [
          grid(
            columns,
            [
              row(["1", "P_PEDIDO", "TYPE OF D_ID", "✓", null, null, null], ref("domain", "D_ID")),
              row(["2", "P_MINIMO", "NUMERIC(18,4)", null, "= 0", null, null]),
            ],
            "Input",
          ),
          grid(columns, [row(["1", "R_TOTAL", "TYPE OF COLUMN PEDIDO_ITEM.PRECO", null, null, null, null])], "Output"),
        ],
      };
    }
    default:
      return {
        shape: "grid",
        sections: [
          grid(
            [col("Property"), col("Value", "code")],
            [row(["Type", "INTEGER"]), row(["Not null", "Yes"]), row(["Default", null]), row(["Check", null])],
          ),
        ],
      };
  }
}

const dataColumns: ColumnInfo[] = [
  { name: "ID", field: "ID", relation: "T", typeName: "INTEGER", kind: "integer", nullable: false },
  { name: "NOME", field: "NOME", relation: "T", typeName: "VARCHAR(120)", kind: "text", nullable: true },
  { name: "VALOR", field: "VALOR", relation: "T", typeName: "NUMERIC(15,2)", kind: "decimal", nullable: true },
  { name: "CRIADO_EM", field: "CRIADO_EM", relation: "T", typeName: "TIMESTAMP", kind: "timestamp", nullable: true },
  { name: "OBS", field: "OBS", relation: "T", typeName: "BLOB SUB_TYPE TEXT", kind: "text_blob", nullable: true },
];
const TOTAL = 5000;
const dataRow = (i: number): Cell[] => [
  String(i + 1),
  i % 7 === 3 ? null : `nome ${i + 1}`,
  ((i * 37) % 100000 / 100).toFixed(2),
  `2026-08-21 15:${String(i % 60).padStart(2, "0")}:52.1234`,
  i % 5 === 0 ? "(MEMO)" : null,
];
const results = new Map<number, ResultView>();
let lastId = 0;

function command(cmd: string, a: Record<string, unknown>): unknown {
  switch (cmd) {
    case "charsets":
      return ["NONE", "UTF8", "WIN1252", "ISO8859_1"];
    case "list_registrations":
      return registrations;
    case "connect":
      registrations[0].server = server;
      return server;
    case "disconnect":
      registrations[0].server = null;
      return null;
    case "explorer_tree":
      return tree;
    case "object_list":
      return objects[a.kind as string] ?? [];
    case "object_tabs":
      return tabsOf(a.kind as string);
    case "object_tab":
      return content(a.kind as string, a.name as string, a.tab as string);
    case "extract_metadata":
      return (a.objects as ObjectRef[]).map((x) => tableDdl.replaceAll("PEDIDO_ITEM", x.name)).join("\n");
    case "editor_begin":
      return ++lastId;
    case "editor_end":
    case "result_close":
      return null;
    case "editor_cancel":
      return false;
    case "sql_statements": {
      const text = a.text as string;
      return text.trim() ? [{ from: 0, to: text.length, end: text.length, kind: { type: "sql" }, unterminatedLiteral: false }] : [];
    }
    case "statement_parameters":
      return { sql: a.sql, names: [] };
    case "editor_execute": {
      const result: ResultView = { id: ++lastId, rows: a.first as number, eof: false, endedEarly: false, fetchMs: 3.2, performance: null, sort: null };
      results.set(result.id, result);
      const executed: Executed = {
        statementType: "select",
        columns: dataColumns,
        result,
        rows: [],
        records: { selected: 0, inserted: 0, updated: 0, deleted: 0 },
        plan: "PLAN (T NATURAL)",
        explainedPlan: null,
        prepareMs: 1.4,
        executeMs: 0.6,
        ended: null,
        performance: null,
      };
      return executed;
    }
    case "result_fetch": {
      const result = results.get(a.result as number)!;
      const rows = Math.min(TOTAL, result.rows + Math.min(a.max as number, TOTAL));
      const next = { ...result, rows, eof: rows >= TOTAL };
      results.set(next.id, next);
      return next;
    }
    case "result_sort": {
      const next = { ...results.get(a.result as number)!, rows: TOTAL, eof: true, sort: a.sort as ResultView["sort"] };
      results.set(next.id, next);
      return next;
    }
    case "result_rows": {
      const result = results.get(a.result as number)!;
      const from = a.from as number;
      const count = Math.max(0, Math.min(a.count as number, result.rows - from));
      const descending = result.sort?.descending ?? false;
      return Array.from({ length: count }, (_, i) => dataRow(descending ? TOTAL - 1 - from - i : from + i));
    }
    case "result_blob":
      return { text: "A note.\nOn two lines.", hex: null, size: 21, complete: true };
    default:
      throw { kind: "invalid", message: `The preview has no answer for ${cmd}.` };
  }
}

mockWindows("main");
mockIPC((cmd, payload) => command(cmd, (payload ?? {}) as Record<string, unknown>));

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <Shell />
  </StrictMode>,
);
