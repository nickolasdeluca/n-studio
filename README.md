# N Studio

**N** — *Any database, One IDE.*

A cross-platform database IDE, native binaries for Linux, macOS and Windows.

Firebird first, modeled on IBExpert: dialects, PSQL, generators, domains, the
services API and the `MON$` tables are first-class, not adapters over a generic
SQL abstraction. PostgreSQL and other engines follow once the Firebird base is
stable, each built to the same depth.

**Status: pre-alpha.** M0 (driver and grid spikes) and M1 (connect + explorer)
are done; M2 (the SQL editor) is in progress — see [docs/PLAN.md](docs/PLAN.md) for the implementation plan and
[docs/M0-FINDINGS.md](docs/M0-FINDINGS.md) for what the spikes established.

## Layout

```
crates/fb-core       connection actor, transactions, cursors, type mapping
crates/fb-meta       version-aware catalog: object kinds, counts, lists
src-tauri/           the app's backend: registrations, keychain, commands
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

## Running the app

```sh
cd ui && npm install
npm run tauri dev      # dev server + app, reloading on change
npm run tauri build    # bundles for this platform
```

Registrations live in the app's config directory (`registrations.json`);
saved passwords live in the OS keychain, never in that file.

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
