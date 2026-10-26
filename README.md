# fbexpert

A cross-platform Firebird database IDE, modeled on IBExpert. Native binaries for
Linux, macOS and Windows.

Firebird-only by design: dialects, PSQL, generators, domains, the services API and
the `MON$` tables are first-class, not adapters over a generic SQL abstraction.

**Status: pre-alpha.** M0 spikes in progress — see [docs/PLAN.md](docs/PLAN.md) for the
implementation plan and [docs/M0-FINDINGS.md](docs/M0-FINDINGS.md) for what the spikes
have established so far.

## Layout

```
crates/fb-core     connection actor, transactions, cursors, type mapping
crates/fb-wire     vendored pure-Rust Firebird wire protocol, extended
spikes/fb-probe    M0 driver validation tool + raw handshake dumper
docker/            Firebird 2.5/3.0/4.0/5.0 test matrix and fixture schema
docs/              plan and spike findings
```

## Running the probe

```sh
FB_HOST=host FB_PORT=3050 FB_DB=/path/to.fdb FB_USER=SYSDBA FB_PASS=pw \
  cargo run -p fb-probe -- --survey        # read-only recon of an existing database

cargo run -p fb-probe -- --perf "select …" # plan + IBExpert-style performance block
cargo run -p fb-probe -- --matrix          # every server in docker/compose.yml
cargo run -p fb-probe --bin handshake      # raw op_connect dump, for auth debugging
```

`FB_AUTH=legacy` selects Legacy_Auth, needed for servers whose SRP user store is
empty. `FBW_TRACE=1` dumps wire packets.

## License

MIT
