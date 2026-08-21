# fbexpert — Implementation Plan (draft for review)

A cross-platform, Firebird-only DBMS IDE modeled on IBExpert. Native binaries for
Linux, macOS and Windows. Not a generic multi-DBMS tool: every feature may assume
Firebird semantics (dialects, PSQL, generators, domains, services API, MON$ tables).

---

## 1. Stack decision

**Recommendation: Rust core + Tauri 2 shell + TypeScript UI.**

| Layer | Choice | Why |
|---|---|---|
| Core / DB | Rust, `rsfbclient` | Has a **pure-Rust wire-protocol** backend — no `fbclient.dll`/`libfbclient.so` to ship or find on the user's machine. Optional native backend for what the wire code can't do. |
| Shell | Tauri 2 | Real native binaries (`.app`/`.dmg`, `.msi`/NSIS, AppImage/`.deb`), ~15 MB, uses system WebView. Rust is the process; the UI is a view. |
| Editor | CodeMirror 6 + custom Lezer grammar for Firebird SQL/PSQL | Folding, multi-cursor, per-dialect tokenizer, context autocomplete, inline error markers. Building this natively is months of work. |
| Grid | Virtualized DOM/canvas grid (custom, on top of TanStack Virtual) | Needs `<null>` rendering, per-type alignment, editable cells, column pinning, blob cells. Off-the-shelf grids fight us on all four. |
| Docking | `dockview` | IBExpert's MDI + dockable panels (Explorer, object list, Messages) map onto it directly. |
| Local storage | SQLite via `rusqlite` | Query history, per-editor performance logs, saved scripts. Registrations in JSON; passwords in OS keychain (`keyring`). |

### Alternatives weighed

- **C++/Qt 6** — closest to IBExpert's feel: `QTableView` with a virtual model handles
  millions of rows for free, QScintilla is a mature editor, KDDockWidgets for docking.
  Cost: C++ velocity, Qt deployment weight, and the DB layer falls back to `fbclient`
  or IBPP, so we ship a native client library per platform. Pick this only if
  webview memory/latency turns out to be disqualifying in M0.
- **Rust + egui** — one static binary, no webview. But the code editor, docking,
  and grid all get written from scratch. Wrong trade for an IDE this dense.
- **Flutter / Electron / .NET MAUI** — either weaker Firebird driver story
  (Flutter FFI to fbclient) or heavier runtime for no gain over Tauri.

### The one real risk

WebView rendering of a 200-column × 100k-row grid. Mitigation is what IBExpert
itself does: never fetch the full set. Cursors stay open and stream in chunks
(default 200 rows), the grid windows the DOM, and "Count records" is an explicit,
opt-in second query — exactly the toolbar toggle in the screenshots. M0 proves this
with a real 1M-row table before we commit.

---

## 2. Crate / module layout

```
fbexpert/
├─ crates/
│  ├─ fb-core/       connection actor, transactions, statement exec, type mapping, cursors
│  ├─ fb-meta/       version-aware RDB$ introspection, DDL generation, dependency graph
│  ├─ fb-sql/        lexer, statement splitter (SET TERM aware), completion context
│  ├─ fb-services/   backup/restore/validate/sweep/stats via Services API
│  └─ fb-monitor/    MON$ tables: attachments, transactions, statements, I/O stats
├─ src-tauri/        commands, app state, cursor registry, config, keychain
└─ ui/               React + TS: explorer, editor, grid, object editors
```

### Threading model

A Firebird attachment is not safe to drive from arbitrary threads. Each connection
gets **one dedicated OS thread running an actor loop**; the UI sends typed requests
over an mpsc channel and gets `oneshot` replies. This gives us, for free:

- cancellation (`fb_cancel_operation` from a control channel while a query runs),
- correct transaction affinity — each SQL editor owns its own transaction, like IBExpert,
- no `Send`/`Sync` gymnastics around the driver.

Open cursors live in a registry keyed by `cursor_id`; the UI asks for
`fetch(cursor_id, n)` as the user scrolls, and the cursor is closed on commit,
rollback, tab close, or LRU eviction.

---

## 3. Firebird-specific engineering (the part a generic tool gets wrong)

- **Dialect 1 vs 3** — dialect 1 has no `BIGINT`, different `DATE` semantics
  (`DATE` == `TIMESTAMP`), double quotes are string literals not identifiers.
  The lexer, DDL generator, and the grid formatter all branch on dialect.
- **Character sets** — connection charset vs per-column charset. `NONE`, `WIN1252`,
  `ISO8859_1` and `UTF8` all have to round-trip; Brazilian legacy DBs are usually
  `WIN1252`. Transliteration errors are the #1 source of "why is this tool broken".
- **Numeric types** — `NUMERIC`/`DECIMAL` are scaled `SMALLINT`/`INTEGER`/`BIGINT`;
  values must never touch `f64`. FB4 adds `DECFLOAT(16|34)`, `INT128`,
  `TIMESTAMP WITH TIME ZONE`.
- **Performance panel** (screenshots 2 and 3) — this is not generic instrumentation:
  - plan from `isc_dsql_sql_info` + `isc_info_sql_get_plan`, and the FB3+ *explained* plan,
  - prepare/execute/avg-fetch time measured around the API calls,
  - reads / writes / fetches / marks by **diffing `isc_database_info` counters**
    before and after execution,
  - current memory / max memory / memory buffers from the same info call.
- **Transaction control** — explicit per-editor transaction with a TIL selector:
  Read Committed (`rec_version` / `no_rec_version`), Snapshot, Snapshot Table Stability,
  read/write vs read-only, wait / no-wait / `lock timeout`. Commit/Rollback and
  Commit Retaining/Rollback Retaining are first-class toolbar actions.
- **Metadata** — all introspection is version-conditional. Packages and identity
  columns are FB3+; `RDB$PROCEDURE_TYPE`, `RDB$FUNCTIONS` semantics, and
  `RDB$SECURITY_CLASSES` differ across 2.5/3/4/5. `fb-meta` keeps one query set
  per server generation, selected from the connection's server version.
- **BLOBs** — segmented, fetched lazily; text vs binary sub-type decides the viewer.
- **Script execution** — `SET TERM` handling is why a real splitter is needed, not
  a `split(';')`.

---

## 4. Milestones

### M0 — Spikes (blocking, ~1–2 weeks)
Prove the risky assumptions before any UI is built:
1. `rsfbclient` pure-Rust backend against Firebird **2.5, 3.0, 4.0, 5.0** in Docker —
   connect, dialect 3, SRP auth, wire compression, `WIN1252` and `UTF8` round-trip.
2. Confirm what pure-Rust **cannot** do (likely: Services API, events, cancel). Decide
   the fallback: optional native `fbclient` backend, or implement those wire ops ourselves.
3. Read the full performance-info block above from a real query.
4. Tauri shell + 1M-row streamed grid; measure scroll latency and memory.

Exit criterion: a throwaway binary that connects, runs `SELECT`, streams the grid,
and prints the same performance block as the screenshot.

### M1 — Connect + Explorer
Database registration dialog (path/alias, host/port, user, role, charset, dialect,
client library override), keychain-stored credentials, connect/disconnect, the object
tree with live counts, and the object-list panel beneath it. Matches screenshot 1's left column.

### M2 — SQL Editor (the core loop)
Multi-tab editor windows, Firebird syntax highlighting, execute / execute-all /
execute-to-cursor, streamed results grid with the record navigator and `<null>`
rendering, Messages / Query columns / Explain plan bottom tabs, Performance Analysis
tab, transaction toolbar + TIL selector, status bar. This is screenshots 2 and 3.

### M3 — Object viewers (read-only)
Per-object-type editors with the IBExpert tab layout: Fields, Constraints, Indices,
Dependencies, Triggers, Data, Description, DDL. Metadata extract for a selection.

### M4 — Metadata editing
DDL generation and compile-back for tables, views, procedures, triggers, generators,
domains, exceptions, roles, grants. Script executor with an error log.

### M5 — Services + monitoring
Backup / restore / validate / sweep / statistics, user manager, `MON$`-based
activity monitor, kill-attachment.

### M6 — Stretch
PSQL debugger (IBExpert's is an *emulator*: it parses the procedure body and executes
statements individually against a private transaction — worth studying before promising),
data comparer, query builder, ER diagrams.

---

## 5. Testing

- `docker compose` matrix of `firebirdsql/firebird:{2.5,3.0,4.0,5.0}`, seeded with a
  fixture DB covering every type, dialect 1 *and* 3, and both charsets.
- Integration tests in `fb-core`/`fb-meta` run against the matrix in CI.
- Golden-file tests for DDL generation per server version.
- Grammar tests for `fb-sql` (statement splitting, `SET TERM`, dialect-1 quoting).

## 6. Non-goals

Other DBMS engines. ORM/codegen. Cloud sync. Pixel-cloning IBExpert's icons or
reusing any of its code or assets — the UI *shape* is fair game, the artwork and
trademark are not.

---

## 7. Frontend decisions (settled)

| Decision | Choice | Consequence |
|---|---|---|
| Framework | React 19 + TS + Vite | dockview, TanStack Virtual and CodeMirror bindings are all React-first. |
| Editor | CodeMirror 6, Lezer grammar for Firebird SQL/PSQL | Lighter than Monaco and cheap to instantiate many times — we open one editor per tab. |
| Look | Modern dense IDE | IBExpert's information density, current visual language: flat panels, hairline borders, system font, real dark mode. No beveled Win95 chrome. |
| Window model | Docked tabs + dockable panels (`dockview`) | One shell. Editors are tabs in a splittable area; Explorer, object list and Messages dock around it. No hand-built MDI. |
| Grid | DOM-virtualized (TanStack Virtual) | Keeps text selection, accessibility, in-place cell editors and blob popups. Viable because results stream in chunks and are never fully materialized. |
| State | Rust owns connections, transactions, cursors and metadata; TS holds view state only (Zustand) | No duplicated source of truth. A tab close or a rollback is a Rust-side event the UI reacts to. |
| Styling | Tailwind, tokens for density and theme | Font size and row height are user-adjustable (screenshot 3 has a grid font-size spinner). |
| Keybindings | IBExpert preset **and** a full keymap editor | F9 execute, Ctrl+F9 prepare, Shift+F9 execute-all, Ctrl+Space completion, F12. Every binding remappable; preset ships as the default so migrating users keep muscle memory. |
| i18n | **Deferred** — en-only through M2 | Accepted debt. Strings go through a single `t()` shim from day one so the retrofit is a catalog extraction, not a UI rewrite. |
| Icons | Custom set, IBExpert icon *grammar* only | Per-object-type icons (table, view, procedure, trigger, generator, domain). None of IBExpert's artwork is reused. |

### Shell layout

```
┌─ menu bar ─────────────────────────────────────────────┐
├─ main toolbar ─────────────────────────────────────────┤
├──────────────┬─────────────────────────────────────────┤
│ Explorer     │ editor toolbar: DB ▾ · run · TIL ▾      │
│ (tree)       ├─────────────────────────────────────────┤
│              │ tab strip: query1 · query2 · PRODUTO    │
│              ├─────────────────────────────────────────┤
│              │ CodeMirror                              │
├──────────────┤                                         │
│ Object list  ├─────────────────────────────────────────┤
│ (grid)       │ Results │ History │ Plan │ Perf │ Log    │
│              │ results grid + record navigator         │
│              ├─────────────────────────────────────────┤
│              │ Messages │ Query columns │ Explain plan  │
├──────────────┴─────────────────────────────────────────┤
│ status bar: cursor pos · DB (dialect) · txn state      │
└────────────────────────────────────────────────────────┘
```

Every region except the menu, main toolbar and status bar is a `dockview` panel:
draggable, floatable, closable, and restored from a persisted layout.

---

## 8. Scope decisions (settled)

**Target servers: Firebird 2.5, 4.0, 5.0.**
Two introspection query sets in `fb-meta`: the legacy 2.5 set (no packages, no identity
columns, legacy auth, older `RDB$` layout) and the FB3+ set. **3.0 falls out nearly free** —
it shares the FB3+ query set and SRP auth with 4.0/5.0 — so it goes in the CI matrix and
gets claimed as supported unless it surprises us. Embedded (serverless file access) is
**out**: it would require bundling the native engine per platform and forfeit the
zero-native-dependency property that motivated the pure-Rust driver.

**M2 grid is read-only.** SELECT, stream, sort, view, export. Cell editing and
INSERT/UPDATE/DELETE from the grid move to M4 alongside the rest of the write path,
where update-statement generation and dirty-row tracking already have to exist.

**Distribution: unsigned CI artifacts until the tool is worth installing.**
GitHub Actions builds macOS/Windows/Linux bundles per commit. No Developer ID,
no Windows cert, no updater plugin before M2 ships. macOS users right-click-open;
Windows shows SmartScreen. Signing and package-manager presence get revisited once
there is something to distribute.

**Open:** license (affects whether `rsfbclient`'s and Qt-adjacent dependencies' terms
matter, and whether this is public from commit one).
