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
- **SRP on servers with wire encryption off.** Such servers answer `op_accept_data`,
  not `op_cond_accept`, and expect the SRP proof in the attach DPB
  (`isc_dpb_specific_auth_data`); upstream always sent `op_cont_auth`, which the
  server refuses with `isc_unavailable`. The handshake also leads with `Srp256`,
  as `fbclient` does.
- **Framed reply reader.** One connection-level buffer; each reply is parsed out
  of it by exact length and the rest kept for the next (`read_reply`). Fixed
  along the way: end-of-cursor left its `messages` word unread, `execute2` skipped
  the `op_response` opcode, and blobs were fetched mid-batch while the rest of the
  batch was still in flight.
- **`BUFFER_LENGTH` = 65535** (upstream: 1 KiB), the cap advertised to the server
  for prepare, info and segment replies. It must fit in a USHORT; 65536 arrived
  as 0 and made blob reads loop forever on empty segments.
- **Exact numerics and Firebird 4 types** (with `crates/fb-wire-core`). Scaled
  integers keep their scale and arrive as `SqlType::Decimal` instead of being
  coerced to `DOUBLE`; `INT128` is read natively (16 bytes, big-endian on the
  wire); `DECFLOAT`, the time-zone types and an untyped `NULL` are asked for as
  text, which the server formats exactly. A `Decimal` parameter goes as a
  scaled `INT64` when it fits, as text otherwise.
- **Statement plan and performance counters** (`info.rs`).
  `RustFbClient::statement_plan` (classic and FB3+ explained) over `op_info_sql`;
  `RustFbClient::database_counters` over `op_info_database`: page reads, writes,
  fetches and marks, memory, buffers, and per-table record counters. Inherent
  methods for now; upstream would want them as a trait in `rsfbclient-core`.
- **`trace()`**, gated behind `FBW_TRACE=1`, dumping wire packets.
