# N Studio — Implementation Plan

**N** — *Any database, One IDE.* N Studio is the application; N is the brand.

A cross-platform database IDE, native binaries for Linux, macOS and Windows.
**Firebird first**, modeled on IBExpert: until that base is stable, Firebird is
the only engine and every Firebird feature may assume Firebird semantics
(dialects, PSQL, generators, domains, services API, MON$ tables). Other engines
— PostgreSQL first — come after, each with the same depth rather than a
lowest-common-denominator SQL layer. See §8, *Engines*.

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
n-studio/
├─ crates/
│  ├─ fb-core/       connection actor, transactions, statement exec, type mapping, cursors,
│  │                 the services manager (backup/restore/validate/sweep/stats)
│  ├─ fb-meta/       version-aware RDB$ introspection, DDL generation, dependency graph,
│  │                 MON$ activity (attachments, transactions, statements), users
│  └─ fb-sql/        lexer, statement splitter (SET TERM aware), PSQL parser for the debugger
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

### M0 — Spikes — done
Prove the risky assumptions before any UI is built:
1. `rsfbclient` pure-Rust backend against Firebird **2.5, 3.0, 4.0, 5.0** in Docker —
   connect, dialect 3, SRP auth, wire compression, `WIN1252` and `UTF8` round-trip.
2. Confirm what pure-Rust **cannot** do (likely: Services API, events, cancel). Decide
   the fallback: optional native `fbclient` backend, or implement those wire ops ourselves.
3. Read the full performance-info block above from a real query.
4. Tauri shell + 1M-row streamed grid; measure scroll latency and memory.

Exit criterion: a throwaway binary that connects, runs `SELECT`, streams the grid,
and prints the same performance block as the screenshot.

### M1 — Connect + Explorer — done
Database registration dialog (path/alias, host/port, user, role, charset, dialect,
auth plugin), keychain-stored credentials, connect/disconnect, the object
tree with live counts, and the object-list panel beneath it. Matches screenshot 1's left column.
The planned *client library override* does not apply: the driver is pure Rust and
loads no `fbclient`; the auth plugin choice took its place.

### M2 — SQL Editor (the core loop) — done
Multi-tab editor windows, Firebird syntax highlighting, execute / execute-all /
execute-to-cursor, streamed results grid with the record navigator and `<null>`
rendering, Messages / Query columns / Explain plan bottom tabs, Performance Analysis
tab, transaction toolbar + TIL selector, status bar. This is screenshots 2 and 3.

In place: all of the above, plus parameters: `?` and `:name` are asked for in a
dialog that shows each one's declared type, remembered per tab; `:name` is bound
by position and is left alone in PSQL, where it is a variable, except in an
`EXECUTE BLOCK`'s input list, whose values come from the client. Also: cancel of a
running statement (`op_cancel`), blobs read on demand in a viewer beside the
grid (as text, a picture, indented JSON or XML, or a hex dump, whichever the
bytes are; a subtype 0 blob that holds text reads as text; saved to a file
whole), commit-or-roll-back prompts before a tab, a connection or the app closes
over an active transaction, sorting by a header click (every row fetched first;
numbers by value, text ignoring case, nulls first) and export to CSV or TSV
(every row, in the grid's order, blobs read in full while the transaction lasts).

### M3 — Object viewers (read-only) — done
Per-object-type viewers with the IBExpert tab layout: Fields, Constraints, Indices,
Dependencies, Triggers, Data, Description, DDL for tables; the source first for
procedures, functions, packages and triggers; properties for the rest. A viewer
opens by a double-click in the explorer or the object list, and a grid row that
stands for another object (a column's domain, a foreign key's table, a dependency)
opens that one. The backend names each kind's tabs and fills them as a few shapes
(grids, SQL, text, a query), so the UI knows nothing of Firebird's catalog. The Data
tab is the editor's grid over a read-only transaction of the viewer's own. Metadata
extract for a selection: several objects picked in the object list become one
script in an order that runs (foreign keys after every table, views after the
views they select from, triggers last). Routines are not ordered by what they
call; that comes with the script executor in M4.

### M4 — Metadata editing — done
Every change goes one way: the backend writes the script that makes it, a dialog
shows the script, and it runs in one transaction, committed whole or rolled back
with the server's errors beside the statements that raised them. What compiles over
itself (views, procedures, functions, packages, triggers) is changed by editing its
DDL tab; a description by editing its tab. The parts of the rest are changed by
actions the backend names for each grid and row, each a form it describes: fields,
constraints and indices of a table, triggers switched on and off, a domain, a
generator's value, an exception's message. New objects start from a template in an
editor; dropping one shows the statement first.

Grants are read from the catalog and grouped back into the statements that gave
them: a Grants tab on what can be granted on, members and privileges on a role,
grant and revoke as actions, and the grants at the end of an object's DDL. Metadata
extract now orders views and routines together by what they use, declaring routines
that call each other with empty bodies before defining them.

The script executor is the SQL editor's "Execute as Script": the statements run one
after the other as isql would (`SET TERM`, `SET AUTODDL`, commits along the way),
with progress, cancelling, stop-on-error and a log of what failed and where.

A table's Data tab is edited in place: new values, deleted rows and new rows are
kept beside the fetched ones (new rows first, where more rows arriving does not move them) and applied as a script like any other change. Rows are
found by a primary key, or a unique one without nulls; a table without either stays
read-only. Blobs are not edited in the grid. Result grids of the SQL editor stay
read-only: a query does not say which table a row belongs to.

### M5 — Services + monitoring — done
Backup / restore / validate / sweep / statistics, user manager, `MON$`-based
activity monitor, kill-attachment.

Done. The pure-Rust driver had no Services API, so the wire client learned it
(`op_service_attach` / `start` / `info` / `detach`, sharing the database attach's
authentication). Backup, restore, validation, sweep, statistics and the server log
run from a registration's menu and write into a tab as they go; every path is a
path on the server. Validation never mends, and is online on Firebird 3 and later.
A restore refuses the path of the database it was started from. Stopping a service
stops listening to it: the Services API has no cancel, so the server may finish
what it began.

The activity monitor reads `MON$` (attachments with their I/O and record counters,
statements, transactions, the database's header counters), by hand or every few
seconds. Disconnecting an attachment and cancelling a statement are scripts, shown
before they run like every other change, as are creating, changing and dropping
users. Users are listed from `SEC$USERS` on Firebird 3 and later and through the
services manager on 2.5.

The two planned crates did not happen: services are a connection, and live with
the connection in `fb-core`; the monitor and the users are catalog readers, and
live in `fb-meta`.

What M5 first left out came after it. Trace sessions are M8. The database's
settings (sweep interval, forced writes, page buffers, read only, reserved
space) are changed through the services manager as `gfix` does, only those
the form changed, read only last since a read-only database takes no other
change. A database is shut down (multi-user, single-user or full; ending
every connection after some seconds, or failing if connections or
transactions remain) and brought online again. Mending (`gfix -mend -full`)
is there for a database that cannot be backed up, with a warning that what
it takes out is lost. `nbackup` backs up by level and puts a database back
together from a level 0 backup and the levels on top of it. And a path with
letters past ASCII names the file it says: on Firebird 3 and later the
service attach says its strings are UTF-8 (`isc_spb_utf8_filename`), where
before the server read them in its own code page; on 2.5, which has no such
flag, a path still has to be in the server's code page. The fixture's live
test does each of them to a restored copy, never to the fixture itself.

### M6 — Stretch — done
PSQL debugger (IBExpert's is an *emulator*: it parses the procedure body and executes
statements individually against a private transaction — worth studying before promising),
data comparer, query builder, ER diagrams.

Done, each as a tab of its own.

**Diagram.** Tables and the foreign keys between them, from one table and what it
is related to or of the whole database, laid out once and then dragged; exported
as SVG. Positions live only as long as the tab, and a whole database's diagram
stops at 250 tables.

**Data comparer.** Two tables' rows, of one database or of two connected ones,
matched by the target's primary key (or a unique key of NOT NULL columns). It
counts every difference, keeps 20,000 in detail, and writes the DELETE, UPDATE
and INSERT statements that make the target the source into an SQL editor; it
changes nothing itself. A blob is compared by a digest the server works out
in the same read (SHA-256 from Firebird 4, a 64-bit hash and the length
before, and said to be), and read only for a row that differs, to write it
into the script in pieces a literal holds; one past 1 MB (16 KB before
Firebird 3) is told to be copied by hand. Computed columns are compared
and never written. Arrays are left out and said to be: no SQL statement
reads or writes one.

**Query builder.** Tables and views picked by name, joined by their foreign keys
when they first enter, columns ticked, renamed, totalled, sorted and filtered. The
backend writes the SELECT, which goes to an SQL editor. It builds one SELECT:
no unions, subqueries, expressions as columns, or OR between filters.

**PSQL debugger.** The server has no debugging interface, so this is an emulator
as IBExpert's is: `fb-sql` takes a procedure's or function's source apart into
statements, `fb-meta` follows the flow, and every expression and SQL statement is
sent to the server in the run's own transaction, with the variables it names as
parameters. Stepping, breakpoints, variables as they change, the rows suspended;
a finished run's transaction is committed or rolled back by whoever debugs, a
failed or stopped one is rolled back.

What that means, said plainly: it runs the source read again, not the compiled
routine, so it shows what the routine does and not what it costs. Routines called
from the debugged one run on the server, and are not stepped into. It refuses, at
the statement, what it does not follow: cursors opened and fetched by hand, `FOR
… AS CURSOR`, `FOR EXECUTE STATEMENT`, `EXECUTE STATEMENT` with parameters or
options, `IN AUTONOMOUS TRANSACTION`, exceptions raised with `USING`; a routine
declaring sub-routines is not opened at all. Triggers (NEW and OLD) and packaged
routines are not debugged. `WHEN ANY` and `WHEN EXCEPTION` handlers work, undoing
their block to a savepoint; `WHEN SQLCODE` / `GDSCODE` / `SQLSTATE` never match,
and the `SQLCODE` and `GDSCODE` variables are not there (`ROW_COUNT` is). A
parameter left empty is NULL, its default not applied; BLOB variables are not
carried. In an expression a bare name is a variable unless it is inside a
subquery, where only `:NAME` is.

### M7 — Alpha — done
M1 to M6 built what the tool does. This one is what stands between that and
handing it to someone who uses IBExpert all day: something to install, and the
parts of the editor's daily loop the feature milestones walked past.

1. **Release bundles.** A workflow that builds the installers for macOS, Windows
   and Linux and attaches them to a GitHub pre-release. On a version tag and by
   hand, not on every commit as §8 first said: three platforms a push is a cost
   nobody reads the result of. Unsigned still; the README says how each system
   is talked into opening it. One version number, the workspace's, starting at
   `0.1.0`, shown in the app so a report can name it.
2. **Completion.** The editor completes keywords and nothing else. It gets the
   catalog `fb-meta` already reads: tables, views, routines, generators, and a
   table's columns after its name or alias.
3. **Scripts as files.** Open a `.sql` into a tab, save a tab to a file; a file
   is written back in the encoding and line endings it was found in (`WIN1252`
   with CR LF as readily as UTF-8). Tabs and their text come back when the app
   is opened again, so quitting asks nothing about them; closing one tab asks
   only when it is a file's and has unsaved changes. Text that was never a
   file is a scratch pad's, and closes without a question.
4. **Query history.** What each editor ran, where, when, how long it took and
   what it failed with, kept per registration in SQLite as §1 planned (the
   latest 2,000), searched and put back into an editor from its History tab.
   A statement is kept as typed, with its parameters' names and not their
   values; a script run is one entry. A statement that sets a password is
   never written, nor one too long to have been typed.
5. **A field run.** The app against real databases, read-only as `AGENTS.md`
   has it: `WIN1252`, dialect 1, catalogs of a thousand tables and more. What it
   finds is fixed here or written down as known.
6. **The README** says what the alpha does, what it does not, and how to
   install it.

Where it stands: 1 to 4 and 6 are in place. Of 5, the backend's part: the live
suites of `fb-core` and `fb-meta`, read-only, against a production ERP
database (Firebird 4.0.4, dialect 3, default character set `NONE`, 1,322
tables, 1,549 procedures) pass, and the completion catalog reads its 1,380
relations, 20,714 columns with their types and 3,062 foreign keys in a little
over a second, over a link of some 95 ms a round trip. The app's own window is
in a tester's hands, on a Firebird 2.5 server too. What that run has found so
far is fixed: a blob over 32 KiB never finished reading on 2.5 (the reply
length overflow of 16 signed bits, now kept under for segments and statement
descriptions alike); and asked for: the blob viewer's views, the completion
list saying what each name is, joins completed from the foreign keys with
an alias, and a code formatter (`fb-sql`'s `format`: token for token the
text it was given, laid out by a style that is exported to a file and
imported from one, so a team formats alike). With that the first tag, `v0.1.0`, is cut. A dialect 1 database and a
`WIN1252` default are still to be run by hand.

Exit criterion: a tester installs a released bundle, registers a database of
their own and works a day in it without the source tree. Met: the owner
works in the released app day to day, on their own databases, and closed
the alpha with 0.7.2. Still to be run by hand, and known: a dialect 1
database, and one whose default character set is `WIN1252`.

Not in the alpha, and said to be: the keymap editor and adjustable font sizes
(§7), languages other than English, signing and an updater, what M5 left out,
and the debugger's limits as M6 lists them. The updater came with M9, and the
keymap, the fonts and pt-BR with M12.

### M8 — The activity monitor as a recording — done

Asked for by the alpha's users, with HQbird's monitoring as the reference:
the monitor of M5 reads `MON$` now and then, so what starts and ends between
two reads is never seen, and nothing it shows adds up to an answer to "what
is slow, and why".

- **A trace session** (`fb-core::trace`): the user trace session of the
  services manager, what `fbtracemgr` starts. The configuration is written
  for the server's version (2.5 has a syntax of its own, and every version
  refuses a parameter it does not know); one database is followed by its
  file name as `mon$database` gives it, escaped as the pattern it is read
  as, since before Firebird 5 an alias never matches. The text is asked for
  every second and handed over as it arrives.
- **The text read back** (`fb-core::trace::parse`): the trace plugin prints
  for people, so the reader goes line by line by what `TracePluginImpl.cpp`
  prints, takes what it knows and keeps the rest. The live tests read what
  2.5, 3.0, 4.0 and 5.0 really write for a statement and check every field.
- **A monitoring session** (`src-tauri/src/trace.rs`): a thread of its own
  from start to stop, the text written as it is to a log per session under
  `traces/<registration>` (the server's format, with `# ` lines of ours
  around it, so any trace tool reads it and so does this one, back into a
  session), the last hundred thousand events kept in memory with what they
  add up to by second and by statement, literals taken out.
- **Memory**: a trace says nothing of it, so `MON$MEMORY_USAGE` is read
  every five seconds on a connection of the session's own, and the most each
  statement was seen holding is kept: how a large sort is found.
- **The panel**: Overview, Live (every event, nested by call), Statements,
  Connections and transactions (M5's grids), Sorts and memory, Recorded
  sessions.

What it cannot do, and says: it is not lossless. The server keeps a
session's text in a buffer (`MaxUserTraceLogSize`, 10 MB) and drops events
when a reader lets it fill; reading without pause makes that rare, the gap
is counted and shown, and nothing prevents it. Only SYSDBA and
administrators see every attachment. The server's CPU, its memory as the
operating system counts it, the lock table and how much of a sort went to
temporary files are not told over the wire at all: HQbird has an agent on
the server for those, and a build of the engine of its own.

What M8 first left out came after it:

- The service attach names the connection's role, so a user who is an
  administrator only through `RDB$ADMIN` traces every attachment.
- A session the server cannot use its configuration for (it would run and
  record nothing) stops, failed, with what the server said.
- Long transactions are among the findings: those open a minute or more
  that hold garbage back (not the read-only read committed ones), read
  with the memory every five seconds, the oldest first.
- The monitoring grids read on a connection of their own, opened the first
  time and dropped when it fails or the database disconnects: a slow read
  of `MON$` neither waits behind the user's work nor holds it up. The
  user's own connection is the one shown as this connection.
- The Firebird 5 profiler (`RDB$PROFILER`), a tab of the monitor: a
  session on the registration's own connection measures what its editors
  run, and the report shows each statement with the routines and triggers
  it called, its plan step by step with opens, fetches and time, and its
  PSQL lines, the costliest first. What it measured is in the database's
  `PLG$PROF_*` tables, deleted only when asked. The fixture's Firebird 5
  test profiles a loop and a join.
- A short run against a real server (Firebird 4, its owner's leave given):
  every event read back with its kind, attachment and user, every
  statement with its time and table counts, no line left unread.

### M9 — Out of the alpha: the app updates itself — done

Releases were pre-releases, each fetched and installed by hand. The app now
asks the latest release whether it is newer, when it opens and when the
version in the status bar is clicked, and offers it there; taken, it is
fetched, checked, put in this build's place and started, after the same
questions a quit asks about transactions and unsaved changes.

It is Tauri's updater plugin: a release carries `latest.json` (the version
and, for each system, a file and its signature), and an install takes only
what is signed with the key whose public half it was built with. The
workflow signs the Windows setup and the AppImage, the Mac that notarizes
signs the app's archive and writes `latest.json` from what the release
holds, and then publishes. Releases are no longer pre-releases:
`releases/latest`, which installs ask, answers only for those.

A `.deb` install replaces itself too, since it came after: the bundler
signs the `.deb` and marks the build with the package it went into, the
release carries the signature, `latest.json` names it as
`linux-x86_64-deb` (and the AppImage as `linux-x86_64-appimage`, keeping
`linux-x86_64` for the installs before it), and the updater installs it
with `dpkg -i` through `pkexec`, or `sudo` where there is none. A `.deb`
install of 0.7 does not know it can, and is updated by hand one last time.

Left out: nothing is fetched before the user says so; and versions before
M9 have no updater, so they are updated by hand one last time.

### M10 — Script Executive — done

A tool for scripts as work in themselves, with IBExpert's Script Executive
as the reference for what it does (not for how it looks): generated scripts
of thousands of statements, each in its own `SET TERM` pair, run all or in
part, from the editor or straight from a file.

- **What a statement is about** (`fb-sql::classify`): the operation, the
  object kind, its name as the catalog stores it and the member below it
  (a comment's column, an index's or a trigger's table), from the first
  words alone; what is not recognised is `Other`, never an error.
  `fb-sql::isql` reads the isql commands a runner follows: `CONNECT`,
  `SET NAMES`, `SET AUTODDL`, `SET SQL DIALECT`, `SET CLIENTLIB`, `INPUT`,
  and IBExpert's `DESCRIBE`, run as the `COMMENT ON` it means.
- **Splitting as it reads** (`fb-sql::Splitter`): one statement's tokens
  held at a time. `split` is the splitter collected; a 256 MB script splits
  in about 2.4 s holding little beyond its text, where whole it took about
  eleven times its size.
- **The runner** (`fb-core::run_executive`): a sibling of the editor's
  `run_script`. It runs the statements chosen and no others (an unchecked
  `SET` or `CONNECT` does not apply), reports every one, and gets its
  connections from a `Connector`, so a script's own `CONNECT` works: the
  transaction on the connection left is ended as the options say, and a
  `CONNECT` that fails leaves no connection rather than the old one. The
  connection and its transaction live in a `Target` the caller shares, to
  cancel the statement running and to end what a run left.
- **A connection of its own** (`src-tauri/src/executive.rs`): a run neither
  queues behind the editors nor they behind it. A `CONNECT`'s password is
  the one it wrote, else that of the registration naming the same server,
  port, database and user (its session's, then the keychain's), else the
  user's, asked while the run waits; it is never logged or kept.
- **The panel**: the script in the editors' CodeMirror view (one factory
  for both now), Statements (checkboxes, state, time and rows of each,
  Check Only Failed, check from a row), Options, an explorer of what the
  script touches by kind, messages by line, Run (F9), Run Checked, Run from
  File (Shift+F9), Stop, Add CONNECT, the database and the character set.
  Opened with Ctrl+F12, as in IBExpert, from the toolbar, or from a
  registration's menu.

Decided, and confirmed by the owner: Add CONNECT writes no
password unless asked (IBExpert writes it in plain text); a transaction
still active at the end is asked about (always committing is what
unattended runs would want, and is an option); Run Checked applies only
the `SET`s and `CONNECT`s checked.

A file from disk is read in the character set the panel chose, unless it
has a byte order mark; with none chosen, as the editor reads files (UTF-8,
else WIN1252). A byte the chosen set has no character for is an error that
says to choose another, not a guess; the panel says what the file was read
as, and reads it again when the set is changed.

`INPUT 'file'` runs the file where it stands, all of it, read as a file
from disk is: a relative name is found in the folder of the script (the
editor's file, or the one run from disk) and, inside a file read so, in
that file's. `INPUT`s nest up to 16 deep, and a file that would read
itself is a failure, not a loop. `SET AUTODDL` and `SET NAMES` in it go on
holding after it, as in isql. Of its statements only what fails, or is not
run for a reason, is listed, with its file and line, on the `INPUT`'s row;
the `INPUT` fails when any of them did, and stops the script with them
when the options say to stop at errors. A script in no folder can only
name files in full.

What it cannot do, and says: a file is run from disk up to 256 MB, and
only its first 200,000 statements are listed (the run goes through all;
only those listed can be left out). The owner decided this stays: a
script that large is run from the file, not picked through. A `CONNECT`
cannot stop to ask what to do with the transaction it leaves: it
commits, unless the options say to roll back at the end. The database
cannot be changed while a transaction a run left is active; the panel says
to end it first. The owner decided this stays too, rather than asking.

Added later, at the owner's request: **a queue of files** (the Queue
tab, `ui/src/scriptQueue.ts`). Script files are run from disk one after
another, each as Run from File runs one, in the order of their names as
migrations are named (a numeric collation: `V1.1__` after `V1__`, `V10__`
after `V2__`), or the user's once a file is moved. It runs from the first
file not run through, stops at a file that fails or is stopped, and can be
told to stop before the next file. A queue cannot stop to ask between
files, so what a file leaves active is committed, unless the options say
to roll back: the same rule a `CONNECT` follows.

Added after: `CREATE DATABASE` and `DROP DATABASE`; `OUTPUT`, which writes
every row of the selects after it to a file; `SHOW`, answered from the
catalog; `SET BAIL`, `EXIT` and `QUIT`; `SHELL`, run only when an option
allows it; a select's first 50 rows under its statement; parameters asked
as the editor asks them; the statements of an `INPUT`'s file in the
Statements tab, to leave some out; and a Compare tab, which shows each
object the script creates or changes as new, the same or different from
the database, with a line diff.

Left out: IBExpert's own language (`IBEBlock`, `ibec_*`), which the owner
decided stays out; isql's other interactive commands (passed over, said
once). Only the statements of a file the script itself reads with `INPUT`
can be left out, not of one that file reads in turn.
Tried by the live tests on the docker fixture, the panel in the browser
preview, and by the owner against a real database in the app; a file near
the 256 MB limit has been split, not run.

### M11 — The agent — done

Claude Code or Codex inside the app, the way an editor hosts them: N Studio
does not talk to a model itself. It runs the user's own `claude` or `codex`,
whichever the panel is set to, signed in to the user's own account, in a
terminal panel, and gives it a way into the databases that the app decides
the shape of. The agent owns the sign-in,
the conversation, the permissions it asks for and the files of the project
folder it is started in, when it is given one; N Studio owns what the agent may know about a
database. That is the line the owner drew: **the agent sees metadata, never
data, unless the user hands it some.**

- **The panel** (`src-tauri/src/agent.rs`, `ui/src/components/AgentPanel.tsx`):
  a pseudo-terminal running `claude` or `codex`, drawn with xterm.js: in a project folder
  the user picks, or, with none, in an empty folder of the app's own
  (`agent/workspace` in its data folder), where it reaches none of the
  user's files and works with the databases only. The app finds the agent as the user's shell would (a login shell
  on macOS and Linux, whose `PATH` a GUI app does not get); without it, the
  panel says how to install it. Closing the panel ends the process.
- **A door of its own** (`src-tauri/src/agent/mcp.rs`): an MCP server on
  `127.0.0.1`, a port the system picks and a token made for each start,
  handed to `claude` with `--mcp-config`, and to `codex` as `-c` overrides
  of its configuration for that run (`mcp_servers.n-studio.*`, the token
  read from an environment variable, the instructions as
  `developer_instructions`), so the user's `~/.codex/config.toml` is never
  written. Nothing but the process the panel started knows the token. What
  reads metadata runs without asking (`--allowedTools` for Claude Code,
  `approval_mode = "approve"` for Codex); everything else is asked for each
  time. Its tools:
  - `list_databases`: every registration, connected or not, with its
    explorer folder; a connected one with server version, dialect and
    character set. No server, path or user: the agent has no need of them.
  - `connect_database`: connects a registration as the explorer does, with
    its saved password or the one the user types when asked; the agent never
    sees it, and the user may decline.
  - `disconnect_database`: refused while the user has a transaction open
    on the database (the agent's own read-only editors aside), a statement
    running, grid changes not applied or an object viewer's changes not
    compiled. Both are asked for by the agent each time.
  - `list_objects`, `get_object_ddl`, `get_dependencies`, `find_columns`:
    what `fb-meta` reads, as the extract and the object viewers show it,
    less what holds values: a generator's current value is left out.
  - `run_query`: one `SELECT` (or `WITH`), and nothing else, in an editor
    the user sees, in a read-only transaction. The rows go to the grid; the
    agent is told the columns and their types, how many rows came, the
    times and the plan, and an error with the values it quotes taken out.
  - `open_in_editor`: SQL for the user to read and run (a change to a table,
    a new field, a script): what writes is run by the user, never by the
    agent.
    A script of many statements, or with `SET TERM` for procedures and
    triggers, opens in a Script Executive instead (`in: script_executive`),
    which runs it as isql does.
  - `count_affected_rows`: how many rows an `UPDATE` or `DELETE` would
    change, without running it: the statement becomes a `SELECT COUNT(*)`
    over its own table, alias and clauses (`WHERE`, `PLAN`, and `ORDER BY`
    with `ROWS`, in a derived table so `ROWS` limits the count), run in a
    read-only transaction that is rolled back. The agent gets the number
    only, and it is asked for each time. Parameters, `WHERE CURRENT OF`,
    `MERGE` and `UPDATE OR INSERT` are refused; triggers and cascades are
    not followed.
  - `get_activity`: what `MON$` says the database is doing now: the
    transaction counters and the gaps between them, attachments counted and
    the busiest by I/O, the oldest active transactions, the statements
    running and memory. No user, machine or program of an attachment.
  - `get_trace_summary`: the newest Activity Monitor session of the
    database (recording, or opened from its log), with its statements
    ranked by cost as the Statements tab has them. The user starts and
    stops a session; the agent cannot, since a trace is load on the server.
  - Every statement text the agent gets from these has its literals
    replaced by `?`, as the trace already keeps them.
  - `get_shared_data`: what the user chose to share, below.
- **Sharing data** is the user's act: the grid's selection (or its fetched
  rows) through *Share with the agent*, which says first that the rows will
  leave the machine for the agent's provider. Shared rows are kept for the
  panel's process only, up to a limit the dialog states, and the agent is
  told an id to read them by.

Decided with the owner: no sign-in of the app's own (a third party may not
offer a Claude subscription's sign-in; the user's `claude` already has one,
and `codex` its own);
metadata only by default; queries the agent runs are shown to the user and
not to it.

### M12 — Preferences: keys, fonts, language — done

What the alpha left out of §7, in one Preferences dialog; each change is in
use at once and kept in the webview's storage.

- **Keymap.** One list of commands (`ui/src/keymap.ts`), each with a scope
  (everywhere, any SQL text, SQL editor, object editor, Script Executive)
  and IBExpert's keys as its defaults. The editors, the viewers, the
  Executive and the window look a key up there when it is pressed, so a
  change needs no reload; the toolbars' tooltips show the keys in use. A
  key is taken by the letter it types, or by its place when Alt changes
  what it types (Option+F on a Mac, AltGr on Windows). The editor adds and
  removes keys by pressing them, and marks one two commands share where
  both work.
- **Fonts.** The window's zoom (the webview's own, so everything scales
  sharp), the SQL text's size and the result grid's, whose rows grow with
  it.
- **Language.** English or Português (Brasil), by default the system's. A
  catalog per language, keyed by the English; texts the backend sends (an
  object's tabs and columns, the forms of its actions) go through it too.
  A Mac's menu bar is made again in the language. It changes on the next
  start, so nothing on screen is left in the language before.

Left out: the keys that move about and edit text (CodeMirror's, the grid's)
stay as they are; the server's messages and the backend's errors stay in
English.

With it, what the alpha said it lacked is in, but for signed Windows and
Linux installers and the debugger's limits: 0.8.0 leaves the alpha, as a
beta.

not to it.

---

## 5. Testing

- `docker compose` matrix of `firebirdsql/firebird:{2.5,3.0,4.0,5.0}`, seeded with a
  fixture DB covering every type, dialect 1 *and* 3, and both charsets.
- Integration tests in `fb-core`/`fb-meta` run against the matrix in CI.
- Golden-file tests for DDL generation per server version.
- Grammar tests for `fb-sql` (statement splitting, `SET TERM`, dialect-1 quoting).

## 6. Non-goals

Other engines before the Firebird base is stable (§8, *Engines*). ORM/codegen. Cloud sync. Pixel-cloning IBExpert's icons or
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
| Keybindings | IBExpert preset **and** a full keymap editor | F9 execute, Ctrl+F9 prepare, Shift+F9 execute-all, Ctrl+Space completion, F12. Every command remappable (M12; the keys that edit text stay CodeMirror's); preset ships as the default so migrating users keep muscle memory. |
| i18n | English text as the key, a catalog per language (M12) | Strings went through a single `t()` shim from day one, so pt-BR was a catalog, not a UI rewrite. A test keeps the catalog in step with the code. |
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
GitHub Actions builds macOS/Windows/Linux bundles per release (M7; first planned
per commit). No Windows cert yet: Windows shows
SmartScreen (the updater came with M9). The macOS installer is the exception since the alpha: signed with
a Developer ID and notarized on a Mac, and added to the release the workflow
drafts with the other two (the README says how). Signing and package-manager presence get revisited once
there is something to distribute.

**Engines: Firebird now, an engine boundary from M1.**
PostgreSQL and others follow once Firebird is stable, so M1 draws the line they
will plug into without building an abstraction for them yet — the second engine
will show what it has to be, and guessing it now would bend Firebird to fit.
What M1 does:
- Registrations carry `engine: "firebird"`; the rest of a registration is
  engine-specific settings, stored as such.
- The Tauri commands and the UI speak engine-neutral data: values, result sets,
  errors, and the explorer tree as kinds-with-counts and object lists described
  by the backend (the UI does not hard-code Firebird's twelve kinds).
- Everything Firebird-specific stays in the `fb-*` crates. A second engine gets
  its own crates beside them, and a thin dispatch layer in `src-tauri`.

**License: MIT** (`LICENSE`). Vendored crates keep upstream's MIT notice. The
repository is private for now; anything written into it follows `AGENTS.md`
(no names or data from real databases).
