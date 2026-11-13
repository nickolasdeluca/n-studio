# N Studio

**N** — *Any database, One IDE.*

A cross-platform database IDE, native binaries for Linux, macOS and Windows.

Firebird first, modeled on IBExpert: dialects, PSQL, generators, domains, the
services API and the `MON$` tables are first-class, not adapters over a generic
SQL abstraction. PostgreSQL and other engines follow once the Firebird base is
stable, each built to the same depth.

**Status: alpha, 0.1.0.** Firebird 2.5, 3.0, 4.0 and 5.0, over the network; the
driver is pure Rust, so there is no `fbclient` to install. See
[docs/PLAN.md](docs/PLAN.md) for the plan and what each milestone built, and
[docs/M0-FINDINGS.md](docs/M0-FINDINGS.md) for what the first spikes
established.

## What the alpha does

- **Databases**: registered once, passwords in the system's keychain; the
  explorer lists every kind of object with its count.
- **SQL editor**: one transaction per tab, with its isolation chosen; execute a
  statement, to the cursor, everything, or as a script (`SET TERM`, errors
  logged, the rest carrying on); parameters asked for by name; results
  streamed into a grid, sorted, exported to CSV; plan and performance
  counters; cancel.
- **Completion** of tables, views, routines and generators, and of columns
  after a table's name or alias.
- **Scripts as files**, read and written back in the encoding and line
  endings they had (`WIN1252` with CR LF as readily as UTF-8). Open tabs come
  back the next time the app opens.
- **Query history** per database, searched from each editor's History tab.
- **Objects**: viewers for every kind, with fields, constraints, indices,
  dependencies, triggers, grants, DDL and data; changes made through forms or
  by editing the DDL, each shown as the script that makes it before it runs;
  a table's rows edited in the grid; metadata extract of a selection in an
  order that runs.
- **Server**: backup, restore, validation, sweep, statistics and the server
  log; users; the activity monitor, with attachments disconnected and
  statements cancelled.
- **Tools**: a diagram of tables and their foreign keys, two tables' rows
  compared and the script that makes one the other, a query builder, and a
  PSQL debugger that steps through a procedure or function.

## What it does not, yet

- Embedded (serverless) databases, and engines other than Firebird.
- A language other than English; adjustable fonts; remapping keys.
- Signed installers and updates from inside the app: a new version is
  downloaded and installed over the old one.
- `nbackup`, shutting a database down, changing its header settings, trace
  sessions.
- The debugger is an emulator and says what it does not follow: triggers,
  packaged routines, cursors opened by hand (the plan's M6 has the list).
- Closing a tab whose text was never saved to a file does not ask first.
  Quitting keeps every tab's text.

## Layout

```
crates/fb-core       connection actor, transactions, cursors, type mapping
crates/fb-meta       version-aware catalog: object kinds, counts, lists
crates/fb-sql        lexer, statement splitter, PSQL parser for the debugger
src-tauri/           the app's backend: registrations, keychain, session, history, commands
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

## Installing

Releases carry one installer per system, built by
[the release workflow](.github/workflows/release.yml) from a `v…` tag. They are
not signed, so each system has to be told once that the app may run:

- **macOS** (`.dmg`, Intel and Apple Silicon in one): drag the app to
  Applications and open it; after the refusal, *System Settings → Privacy &
  Security → Open Anyway*. Or, in a terminal:
  `xattr -dr com.apple.quarantine "/Applications/N Studio.app"`.
- **Windows** (`…-setup.exe`): SmartScreen's *More info → Run anyway*. The
  installer fetches the WebView2 runtime where Windows does not have it yet.
- **Linux**: `sudo apt install ./n-studio_….deb`, or make the `.AppImage`
  executable and run it. Saved passwords need a Secret Service keyring (GNOME
  Keyring, KWallet).

The status bar shows the version; name it when reporting something.

## Running the app

```sh
cd ui && npm install
npm run tauri dev      # dev server + app, reloading on change
npm run tauri build    # bundles for this platform
```

The app keeps three files in its config directory: `registrations.json`,
`session.json` (the open editors and their text) and `history.db` (what was
run, without parameter values and without statements that set a password).
Saved passwords live in the OS keychain, never in a file.

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

## License

MIT
