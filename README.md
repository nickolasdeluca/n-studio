<div align="center">

<img src="src-tauri/icons/icon.svg" alt="N Studio" width="112" height="112">

# N Studio

**Any database, One IDE.**

[![CI](https://github.com/nickolasdeluca/n-studio/actions/workflows/ci.yml/badge.svg)](https://github.com/nickolasdeluca/n-studio/actions/workflows/ci.yml)
![Platforms](https://img.shields.io/badge/platform-macOS%20%7C%20Windows%20%7C%20Linux-black)
![Status](https://img.shields.io/badge/status-beta-yellow)
[![License](https://img.shields.io/badge/license-MIT-green)](LICENSE)

**A native desktop database IDE for writing SQL, exploring schemas and managing
Firebird servers. Built with Rust, Tauri and React.**

[Download](https://ndworks.dev/n-studio/download) ·
[Features](#working-with-firebird) · [Building](#building) ·
[Roadmap](docs/PLAN.md)

</div>

Firebird first, with the depth of tools like IBExpert: dialects, PSQL,
generators, domains, the services API and the `MON$` tables are first-class.
The driver speaks the Firebird wire protocol in pure Rust — no `fbclient`
to install.

**Alpha · 0.1.0.** Connects to Firebird 2.5, 3.0, 4.0 and 5.0 over the network.
PostgreSQL and other engines are planned once the Firebird base is stable,
each built to the same depth.

## Download

[![Download N Studio](https://img.shields.io/badge/Download-N%20Studio-2f6fdf?style=for-the-badge)](https://ndworks.dev/n-studio/download)

Choose the installer for your system on the releases page. The macOS build
is signed and notarized; Windows and Linux builds are unsigned.

- **macOS** (`.dmg`, Intel and Apple Silicon in one): drag the app to
  Applications and open it.
- **Windows** (`…-setup.exe`): SmartScreen's *More info → Run anyway*. The
  installer fetches the WebView2 runtime where Windows does not have it yet.
- **Linux**: `sudo apt install ./n-studio_….deb`, or make the `.AppImage`
  executable and run it. Saved passwords need a Secret Service keyring (GNOME
  Keyring, KWallet).

The status bar shows the version; name it when reporting something.

## Working with Firebird

### Write and run SQL

Each editor tab has its own transaction and isolation setting, and up to ten
pages of text, as a browser's tabs (Ctrl+T, Ctrl+Tab; a double click renames
one), that share them and its results. Run one statement, everything up to the
cursor, the whole page, or a script with `SET TERM` support and an error log.
Named parameters are prompted for before execution, an `EXECUTE BLOCK`'s
input parameters among them. Results stream into a
sortable grid with CSV export and a summary row (Σ): under each column, chosen
there, the count of its values or of the different ones, the least, the
greatest, and for numbers the sum and the average, of every row. An object
viewer's Data tab has it too. A right click on a row scripts it, or every row,
as INSERT statements, into an editor or a file: from a select, of the columns
it selected from the table the clicked column comes from; from a table's Data
tab, every column of the table, read from the table itself, blobs included. The plan, performance counters and cancellation
stay close to the query, below a bottom that can be minimized. A rollback
takes the rows it rolled back away, so they are not edited afterwards.

Completion knows tables, views, routines, generators and column types,
including columns behind an alias; a table's columns start with its primary
key, in the key's own order. The names of tables, views, routines, triggers
and other objects, and the aliases that stand for them, are drawn as links:
double-click one to open it. After `JOIN`, it can write the join
condition from a foreign key, alias included. The formatter handles SQL and
PSQL with a style you can export and share: keyword case, indentation,
wrapping and list layout, without changing the tokens.

### Keep your work

Scripts retain their encoding and line endings when saved — `WIN1252` with
CR LF as well as UTF-8. Each database keeps its workspace, as IBExpert does:
its SQL editors close when it disconnects and come back, every page and
the one in front, when it connects again, in the same run or a later one.
Each editor's History tab searches queries for that database.

Blobs load on demand beside the grid as text, images, formatted JSON or XML,
or hex. Save the full value to a file when you need it outside the app.

### Make it yours

Preferences (the sliders on the toolbar, or Ctrl+, / Cmd+,) change three
things, in use at once and kept on this machine:

- **Keyboard**: every command of the app, IBExpert's keys by default (F9,
  Shift+F9, Ctrl+F9, F12, Ctrl+Space…). Add a key by pressing it, remove
  one, or reset; a key two commands share where both work is marked.
- **Fonts**: the window's zoom (also Ctrl+= and Ctrl+-), the SQL text's
  size, and the result grids', whose rows grow with it.
- **Language**: English or Português (Brasil); by default the system's.
  It changes the next time the app opens.

### Explore and change a schema

Register a database once; saved passwords go in the system keychain. Clone
a registration to register another database on the same server: the form
opens filled in, password included, for you to change the path or alias. The
explorer lists object kinds with their counts, and object viewers expose
fields, constraints, indices, dependencies, triggers, grants, DDL and data;
click a column header to sort any of their lists.

Edit through a form or directly in DDL. Schema changes show the script before
it runs. Table rows can be edited in the grid, and selected objects can be
extracted as metadata in dependency order.

### Manage the server

Run backup, restore, validation, sweep and statistics, read the server log,
and manage users. Change a database's settings (sweep interval, forced
writes, page buffers, read only, reserved space), shut it down and bring it
online, mend a corrupt one, and back it up and restore it with `nbackup`.
Paths on the server can have accents on Firebird 3 and later.

The activity monitor records everything the server runs, through a trace
session: every statement, procedure, trigger and transaction, with its plan
and what it cost. It shows what that adds up to (charts by the second, the
statements that cost the most, tables read whole, sorts that held a lot of
memory, transactions left open long enough to hold garbage back), lists
each event nested under what called it, and saves each recording to a log
file that can be opened again. It also shows who is connected now, read on
a connection of its own, with actions to disconnect an attachment or cancel
a statement. The server may drop events when it is busier than they can be
read; the monitor counts those gaps. On Firebird 5 its Profiler measures
what the editors run, by line of PSQL and by step of each plan.

### Run scripts

The Script Executive runs scripts as work in themselves, such as the
upgrade scripts tools generate, with each statement in its own `SET TERM`
pair. It opens from the toolbar, a database's menu or Ctrl+F12, on a connection
of its own, so a long script does not hold up the editors. Run the whole script
(F9), only the statements checked in its list, or a file straight from disk
without loading it (Shift+F9, up to 256 MB). Each statement shows its state,
time and rows as it runs; failures are listed by line, and an explorer shows
what the script touches, by object kind, before anything runs.

`INPUT 'file'` runs another script where it stands, found beside the one
that names it; a failure in it is told with its file and line. Its
statements show under it in the list, to leave some out.

isql's own commands work as they do there: `CREATE DATABASE` and `DROP
DATABASE`, `OUTPUT` to write rows to a file, `SHOW`, `SET BAIL`, `EXIT` and
`QUIT`. `SHELL` runs only when the options allow it. A select shows its
first rows under its statement, and parameters are asked before the
statement runs. The Compare tab shows, before anything runs, which objects
the script creates, changes or leaves as they are, with each change line
by line.

A script can name its own database with `CONNECT` and `SET NAMES`; Add
CONNECT writes those lines for a registered database. The password is not
written unless you ask: the run takes it from the keychain, or asks for it.
Options decide whether to stop at the first error, commit DDL as it runs,
and what to do with a transaction still active at the end.

The Queue tab runs many script files from disk, one after another: the
migrations of a release, say. Files added are put in the order of their
names, numbers read as numbers (`V2__` before `V10__`), and can be moved
or removed while they are not running. Run Queue goes from the first file
not run through and stops at one that fails; run it again, once that file
is fixed, to go on from there. Stop After This File lets the file running
end first; Stop stops it too. What a file leaves active is committed
before the next, unless the options say to roll back.

### Work across tables and routines

View tables and foreign keys in a diagram, assemble SQL with the query
builder, or compare two tables' rows and generate a synchronization script.
The PSQL debugger steps through a procedure or function; its emulation
limits are listed below.

### Work with an AI agent: Claude Code or Codex

The AI Agent panel (toolbar, or View) runs your own
[Claude Code](https://claude.com/claude-code) or
[Codex](https://github.com/openai/codex), whichever you choose in it,
signed in as you signed it in.
Without a project it works with the databases only, in an empty folder of
the app's own; choose a project folder to let it read and change that
project's files too. N Studio does not sign in to anything itself.
It gives the agent tools that read the metadata of the connected databases:
objects, their DDL, dependencies, and where a column is. It can see which
databases are registered and ask to connect one (you type the password if
it is not saved) or disconnect one, which it cannot do while you have a
transaction or changes open on it. It can also read what the Activity Monitor
sees: the transaction counters, connections counted, statements running and
the statements a recording found costliest, always with their values
replaced by `?` and without who is connected from where. The agent never
sees the data:

- A query it runs is one `SELECT`, in an editor you see, in a read-only
  transaction. The rows go to your grid. The agent is told the columns,
  the row count, the times and the plan.
- SQL that changes something is opened in an editor for you to read and run;
  a script with many statements or `SET TERM`, in a Script Executive. Before
  handing you an `UPDATE` or `DELETE` it can count the rows it would change,
  without running it: it is told the number, nothing of the rows.
- Rows reach it only when you share them: *Share with Claude Code* (or
  *Codex*) on a grid,
  the current record or up to 1,000 fetched rows. The dialog says first that
  they leave this computer.

The agent must be installed and signed in once from a terminal (`claude`,
or `codex`). Codex is given N Studio's server for that run only, with `-c`
overrides; your `~/.codex/config.toml` is not changed.

## Known limits

- Embedded (serverless) databases, and engines other than Firebird.
- Windows and Linux installers are unsigned.
- The app updates itself from the latest release: it asks when it opens,
  and offers the new version in the status bar (the version shown there
  asks again when clicked). On Linux the AppImage and the `.deb` replace
  themselves, the `.deb` after asking for the administrator's password.
  A `.deb` of 0.7 or before is updated by hand one last time.
- The server's CPU, memory and lock table are not shown: Firebird does not
  report them to a client.
- The debugger is an emulator and says what it does not follow: triggers,
  packaged routines, cursors opened by hand (the plan's M6 has the list).
- Closing a tab whose text was never saved to a file does not ask first.
  Quitting keeps every tab's text.

## Building

Install Rust and Node.js first, along with the platform dependencies used
by [the release workflow](.github/workflows/release.yml). Then, from the
repository root:

```sh
cd ui
npm ci
npm run tauri dev      # dev server + app, reloading on change
npm run tauri build    # bundles for this platform
```

## Local storage

The app keeps four files in its config directory: `registrations.json`,
`session.json` (the open editors and their text), `history.db` (what was
run, without parameter values and without statements that set a password)
and `formatter.json` (the code formatter's style, once one is saved).
The preferences (keys, fonts, language) are kept in the window's own
storage.
Saved passwords live in the OS keychain, never in a file.

## Architecture

```
crates/fb-core       connection actor, transactions, cursors, type mapping
crates/fb-meta       version-aware catalog: object kinds, counts, lists
crates/fb-sql        lexer, statement splitter, PSQL parser for the debugger
src-tauri/           the app's backend: registrations, keychain, session, history, commands,
                     the agent's terminal and its MCP server
ui/                  the app's interface: React, dockview, Tailwind, Zustand
crates/fb-wire       vendored pure-Rust Firebird wire protocol, extended
crates/fb-wire-core  vendored core types for it: exact decimals, Firebird 4 types
spikes/fb-probe      M0 driver validation tool + raw handshake dumper
spikes/grid-shell    M0 Tauri shell + virtualized grid, with an unattended benchmark
spikes/grid-slint    M0 the same grid in Slint, for comparison
spikes/rowstore      Paged scroll-back row store shared by both grid spikes
docker/              Firebird 2.5/3.0/4.0/5.0 test matrix and fixture schema
docs/                plan and spike findings
```

See [the roadmap](docs/PLAN.md) for milestones and
[the initial findings](docs/M0-FINDINGS.md) for the driver and UI spikes.

## Running the probe

```sh
FB_HOST=host FB_PORT=3050 FB_DB=/path/to.fdb FB_USER=SYSDBA FB_PASS=pw \
  cargo run -p fb-probe -- --survey        # read-only recon of an existing database

cargo run -p fb-probe -- --perf "select …" # plan + IBExpert-style performance block
cargo run -p fb-probe -- --matrix          # every server in docker/compose.yml
cargo run -p fb-probe --bin handshake      # raw op_connect dump, for auth debugging
```

`FB_AUTH=legacy` makes the tools lead with Legacy_Auth, for servers whose SRP
user store is empty; in the app it is a per-registration setting. `FBW_TRACE=1` dumps wire packets.

## Making a release

The Developer ID certificate lives in a keychain, not in the repository, so
the workflow cannot make the macOS installer: its own `.dmg` is signed ad hoc,
built only to know that the bundle builds, and never published. A release is
made in two steps:

```sh
git tag v0.1.0 && git push origin v0.1.0    # the version in Cargo.toml
scripts/build-release.sh --release          # on a Mac with the certificate
```

The tag has the workflow build the Windows and Linux installers into a
**draft** release. The script builds the universal `.dmg`, signs it and
the app with the Developer ID, has Apple notarize it and staples the answer;
then it adds the `.dmg` to that draft and publishes it. It refuses a commit
that is not the tagged one, an installer Apple has not notarized, and a draft
the Windows installer has not reached, so a release is never seen with a
system missing. `--release-only` adds the `.dmg` already in `dist/`, for when
the workflow was still running.

Publishing is also what offers the version to every install. Each installer
an install updates itself from (the Windows setup, the AppImage, the `.deb`,
and the macOS app as an archive) is signed with the updater's key: the workflow's
`TAURI_SIGNING_PRIVATE_KEY` secret, and the same key in a file on the Mac
(`UPDATER_KEY_PATH`, `~/.tauri/n-studio-updater.key` by default). The script
writes `latest.json`, with each system's file and signature, adds it and
publishes the release as the latest one; installs ask for that file. The key
is made once (`tauri signer generate`) and kept: an update signed with
another is refused by every install there is.

Once the release is published, the script deletes what it built for it: the
bundle folder under `target/`, whose `.app` Spotlight would otherwise offer
beside the installed one, and the version's files in `dist/`. The compiled
crates stay in `target/`, so the next build starts from them.

Without arguments the script only builds, signs and notarizes, into `dist/`
(`cp scripts/build-release.env.example scripts/build-release.env` once, and
fill it in).

On Windows, `.\scripts\build-release.ps1` builds the setup the workflow builds
and leaves it in `dist\`. It is not signed: there is no Authenticode
certificate yet.

## License

[MIT](LICENSE)
