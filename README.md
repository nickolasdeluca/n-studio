<div align="center">

<img src="src-tauri/icons/icon.svg" alt="N Studio" width="112" height="112">

# N Studio

**Any database, One IDE.**

[![CI](https://github.com/nickolasdeluca/n-studio/actions/workflows/ci.yml/badge.svg)](https://github.com/nickolasdeluca/n-studio/actions/workflows/ci.yml)
![Platforms](https://img.shields.io/badge/platform-macOS%20%7C%20Windows%20%7C%20Linux-black)
![Status](https://img.shields.io/badge/status-alpha-orange)
[![License](https://img.shields.io/badge/license-MIT-green)](LICENSE)

**A native desktop database IDE for writing SQL, exploring schemas and managing
Firebird servers. Built with Rust, Tauri and React.**

[Download](https://github.com/nickolasdeluca/n-studio/releases) ·
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

[![Download N Studio](https://img.shields.io/badge/Download-N%20Studio-2f6fdf?style=for-the-badge)](https://github.com/nickolasdeluca/n-studio/releases)

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

Each editor tab has its own transaction and isolation setting. Run one
statement, everything up to the cursor, the whole editor, or a script with
`SET TERM` support and an error log. Named parameters are prompted for before
execution. Results stream into a sortable grid with CSV export; the plan,
performance counters and cancellation stay close to the query.

Completion knows tables, views, routines, generators and column types,
including columns behind an alias. After `JOIN`, it can write the join
condition from a foreign key, alias included. The formatter handles SQL and
PSQL with a style you can export and share: keyword case, indentation,
wrapping and list layout, without changing the tokens.

### Keep your work

Scripts retain their encoding and line endings when saved — `WIN1252` with
CR LF as well as UTF-8. Open tabs return on the next launch, and each editor's
History tab searches queries for that database.

Blobs load on demand beside the grid as text, images, formatted JSON or XML,
or hex. Save the full value to a file when you need it outside the app.

### Explore and change a schema

Register a database once; saved passwords go in the system keychain. The
explorer lists object kinds with their counts, and object viewers expose
fields, constraints, indices, dependencies, triggers, grants, DDL and data.

Edit through a form or directly in DDL. Schema changes show the script before
it runs. Table rows can be edited in the grid, and selected objects can be
extracted as metadata in dependency order.

### Manage the server

Run backup, restore, validation, sweep and statistics, read the server log,
and manage users. The activity monitor shows attachments and statements,
with actions to disconnect an attachment or cancel a statement.

### Work across tables and routines

View tables and foreign keys in a diagram, assemble SQL with the query
builder, or compare two tables' rows and generate a synchronization script.
The PSQL debugger steps through a procedure or function; its emulation
limits are listed below.

## The alpha caveats

- Embedded (serverless) databases, and engines other than Firebird.
- A language other than English; adjustable fonts; remapping keys.
- Windows and Linux installers are unsigned. There is no in-app updater;
  download a new version and install it over the old one.
- `nbackup`, shutting a database down, changing its header settings, trace
  sessions.
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
Saved passwords live in the OS keychain, never in a file.

## Architecture

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
**draft** pre-release. The script builds the universal `.dmg`, signs it and
the app with the Developer ID, has Apple notarize it and staples the answer;
then it adds the `.dmg` to that draft and publishes it. It refuses a commit
that is not the tagged one, an installer Apple has not notarized, and a draft
the Windows installer has not reached, so a release is never seen with a
system missing. `--release-only` adds the `.dmg` already in `dist/`, for when
the workflow was still running.

Without arguments the script only builds, signs and notarizes, into `dist/`
(`cp scripts/build-release.env.example scripts/build-release.env` once, and
fill it in).

On Windows, `.\scripts\build-release.ps1` builds the setup the workflow builds
and leaves it in `dist\`. It is not signed: there is no Authenticode
certificate yet.

## License

[MIT](LICENSE)
