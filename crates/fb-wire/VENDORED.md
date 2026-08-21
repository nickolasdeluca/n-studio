# crates/fb-wire

A vendored copy of [`rsfbclient-rust`](https://github.com/fernandobatels/rsfbclient)
0.27.0 (MIT), the pure-Rust Firebird wire protocol client.

It lives here because fbexpert needs wire operations the upstream trait surface
cannot express — `op_info_sql` (plan), `op_info_database` (performance counters),
`op_cancel`, and the services API all need raw socket access. Upstream is actively
maintained and merges outside contributions, so each addition should go back as a
PR; this copy is how they ship in the meantime, wired in via `[patch.crates-io]`
in the workspace manifest.

## Changes so far

- **Legacy_Auth support.** `AuthPluginType::Legacy`, advertised in the plugin list;
  `connect()` can lead with it and carry the DES-crypted password as the plugin's
  specific data; `parse_accept` tolerates the empty plugin name a server sends when
  it has already authenticated the connection. Required by Firebird 3/4/5 servers
  kept compatible with 2.5-era applications.
- **Larger `BUFFER_LENGTH`** (1 KiB to 64 KiB), which is also the cap advertised to
  the server for prepare and info responses.
- **`trace()`**, gated behind `FBW_TRACE=1`, dumping wire packets.

## Known defect, not yet fixed

The response reader has no framing — see `docs/M0-FINDINGS.md` §4. Wide `select *`
desyncs the parser and BLOB reads hang. Fixing this is the first task on the crate.
