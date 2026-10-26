# M0 — Spike findings

Status: **partially complete.** Driver capability audit done by source inspection, and
validated against two live Firebird 4.0.4 servers (read-only). The version matrix
still needs a container runtime, which this dev machine cannot run. The blocking
defects of §3 and §4 are fixed — see §8.

---

## 1. Driver capability audit — `rsfbclient` 0.27.0

Crate layout: `rsfbclient` (high level) over `rsfbclient-core` (traits, types, charsets,
`ibase` constants) with two interchangeable backends, `rsfbclient-rust` (pure-Rust wire
protocol, ~5.5k lines) and `rsfbclient-native` (FFI to `fbclient`).

**Upstream is alive** — last commit 2026-08-12, external PRs merged through August 2026
(TCP_NODELAY on the pure-Rust socket, `WireCrypt=Disabled` support). So the strategy is
**extend and upstream**, not hard-fork. MIT-licensed, same as us.

### What works out of the box

| Capability | Where | Note |
|---|---|---|
| Pure-Rust wire protocol | `rsfbclient-rust` | No `fbclient` needed. Protocol V10–V13 offered at handshake; FB4/5 negotiate down to V13. |
| SRP / SRP256 auth | `srp.rs` | FB3+ path. |
| Legacy auth | `wire.rs:236-255` | V10 sends plaintext `isc_dpb_password`; V11/V12 send `unix_crypt`-hashed `isc_dpb_password_enc`. **This is the FB 2.5 path and it exists** — but the initial `op_connect` always advertises `Srp,Srp256` in the uid block, so whether a 2.5 server tolerates it is unverified. First thing to test once a server exists. |
| Batched fetch | `client.rs:28-52` | `op_fetch` pulls 200 rows per round trip (`FB_FETCH_BATCH` env override), surfaced through a row-at-a-time API. **Our chunked-streaming design needs no driver change.** |
| Charsets | `rsfbclient-core/charset.rs` | Connection charset incl. `WIN1252`, `UTF8`, `NONE`. |
| Transaction isolation config | `TransactionConfiguration` | Maps to the TPB; TIL selector is expressible. |

### What is missing — and it is exactly our headline features

The backend trait surface (`FirebirdClientDbOps` + `FirebirdClientSqlOps`) is:
attach / detach / drop / create · begin_transaction / transaction_operation ·
exec_immediate / prepare / free / execute / execute2 / fetch.

Nothing else. In particular:

| Gap | Evidence | Blocks |
|---|---|---|
| **No `isc_database_info` / `op_info_database`** — *added, §9* | `InfoDatabase = 40` exists as a wire opcode constant in `rsfbclient-rust/consts.rs:61`, unused. Native FFI decl is commented out. | Reads / writes / fetches / marks counters, current & max memory, memory buffers — **the entire Performance Analysis panel**. |
| **No `isc_dsql_sql_info` / `op_info_sql` exposed** — *added, §9* | Native backend calls it privately for statement type and affected rows; no public surface, no plan request anywhere. `isc_info_sql_get_plan` never referenced. | `PLAN` text and the FB3+ explained plan — **the Plan Analyzer and Explain plan tabs**. |
| **No Services API** | Only error strings and the opcodes `ServiceAttach/Detach/Info/Start = 82..85` in `consts.rs:109-112`. Native FFI decls commented out. | M5 entirely: backup, restore, validate, sweep, statistics, user manager. |
| **No cancel operation** | `Cancel = 91` opcode constant only; `fb_cancel_*` constants in core, unused. | Stopping a runaway query. Non-negotiable for an IDE. |
| **Events are native-only** | `FirebirdClientDbEvents` implemented solely in `rsfbclient-native/connection.rs:601`. | `POST_EVENT` listeners on the pure-Rust backend. |

### Consequence

Four of the five gaps sit *below* the trait boundary — they need raw socket access, so
they cannot be bolted on from outside the crate. We take ownership of the wire layer:

1. Vendor `rsfbclient-rust` as a git dependency on our own branch (`crates/fb-wire`).
2. Implement, in rough priority order: `op_info_sql` (plan + statement info) →
   `op_info_database` (performance counters) → `op_cancel` → events → `op_service_*`.
3. Upstream each as a PR. The maintainer merges outside contributions; carrying these
   permanently is the fallback, not the plan.

This is additive work on a well-structured 5.5k-line crate that already owns framing,
SRP, BLR and XSQLDA — not a rewrite. But it is real scope that the original plan did
not price in, and it lands on the critical path for M2's performance panel.

---

## 2. Live-server results — Firebird 4.0.4

Target: a real ERP database (COPAZA, 1331 tables) over a ~50 ms link. Everything below
was read-only: `SELECT`s and `RDB$`/`MON$` reads, no writes, no DDL.

```
ENGINE_VERSION    4.0.4          ODS 13.0
default charset   NONE           page size 16384      dialect 3
```

### What works

- **Connect**, after the auth fix in §3.
- **Introspection.** The Explorer tree counts came back matching the IBExpert
  screenshot: Views 4, Procedures 1577, Generators 730, Exceptions 1, Roles 2
  (Tables 1331 / Domains 120 / Triggers 1763 / Indices 4726 differ by a handful
  from the screenshot — IBExpert filters system objects slightly differently and
  the schema has moved since). The introspection queries are sound.
- **Streaming.** The screenshot's own query — six columns from `PRODUTO` —
  returned **5000 rows in 81 ms, ~61k rows/s**, first row at 54 ms. Prepare plus
  execute was 52 ms, i.e. one round trip. No driver change needed for chunked
  streaming.
- **Charsets.** Connection charset `WIN1252` against a `NONE`-charset database:
  zero replacement characters across 5000 rows of accented Portuguese text.
- **Type mapping.** `INTEGER` (raw 580), `VARCHAR` (448) and `TIMESTAMP` (510)
  all mapped correctly, sub-millisecond values included.

### Timing note for M1

Each tree-count query costs one round trip (~95 ms here); ten of them run
sequentially cost ~1 s. The Explorer must issue these as **one `UNION ALL`
query**, not ten, or expanding a database will feel broken on a WAN link.

---

## 3. Auth: the pure-Rust driver could not connect at all

Symptom: every attempt failed with `sql error -1: unavailable database`,
identically for a wrong password and a nonexistent database path — which is the
tell that it fails before either is checked.

Raw wire tracing (`spikes/fb-probe/src/bin/handshake.rs`, written for this)
showed the server replying `op_cond_accept`, protocol 13, plugin **Srp256**, and
then rejecting the SRP continuation with `isc_unavailable`. A hand-built
`op_connect` + `op_attach` carrying plaintext credentials succeeded against the
same server, and a bogus path correctly returned `isc_io_error` — proving the
server was fine and the driver was not.

Cause: **the server authenticates via `Legacy_Auth`**, and `rsfbclient` knows only
`Srp` and `Srp256`. The user confirmed separately that SRP needs a different
password, so both stores exist, but the SRP path failed even with the correct
one — that specific failure is still unexplained and needs a second FB4 server
to compare against.

Fixed in the vendored crate (`crates/fb-wire`), in three parts:

1. `AuthPluginType::Legacy` ("Legacy_Auth"), added to the advertised plugin list.
2. `op_connect` can now **lead** with Legacy_Auth, carrying the DES-crypted
   password as the plugin's specific data the way `fbclient` does. Selected with
   `FB_AUTH=legacy` for now; this becomes a per-registration connection setting.
3. `parse_accept` no longer rejects the reply when the server reports "already
   authenticated" with an empty plugin name.

**This matters well beyond one server.** Legacy_Auth is standard on Firebird 3/4/5
installations kept compatible with 2.5-era applications — which is most of the
target install base.

---

## 4. Blocking defect: the response reader has no framing

Two symptoms, one cause:

- `select * from PRODUTO` (174 columns) fails with `Connection rejected with
  code 1` — the parser reads a bogus opcode.
- Selecting a **BLOB** column (`select ID_PRO, FOTO_PRO from PRODUTO`) **hangs
  forever**, blocking on bytes that already arrived.

Firebird responses carry no transport-level length prefix, and with `ptype_lazy_send`
(negotiated here) the server coalesces several responses into one TCP segment.
`read_packet` does a single `socket.read`, parses one response out of it, and
**discards the rest of the buffer**. Whatever was coalesced behind it is lost, and
the stream desyncs. A short read has the mirror problem: the parser sees a partial
packet and has no way to ask for more.

The `fetch` path was already fixed upstream with a retry-on-`NeedMore` loop, which
is why plain narrow `SELECT`s work. Everything else — prepare, info, blob segments —
still uses the naive reader.

Attempted and rejected, both recorded so they are not retried:

- *Retain the unparsed tail.* Responses are followed by 4-byte alignment padding,
  so "everything the parser did not consume" is not the leftover; feeding it back
  prefixes the next packet with zero bytes and breaks queries that previously worked.
- *Cap the protocol type at 4 (`batch_send`) to disable lazy send.* Made it worse:
  the client's response handling assumes lazy_send is on.

**The real fix is to frame each opcode by its exact wire length** and keep a
connection-level buffer that survives between reads. That is the wire-layer rewrite
the plan already anticipated, and it is now the first item on it rather than a
later one. Estimate: a few days, contained to `crates/fb-wire`, with the raw
handshake tool and the live server available to verify against.

Until it lands, the driver handles narrow, BLOB-free result sets only.

---

## 8. Resolution (2026-10-26)

Against a second FB 4.0.4 server (the ERP database, 1322 tables), cross-checked by reading
the same queries with an independent pure-Python driver (`firebirdsql`):

**§3, SRP "unavailable database" — not a server problem.** Servers with wire
encryption disabled accept with `op_accept_data` (94), not `op_cond_accept` (98),
and take the SRP proof in the attach DPB as `isc_dpb_specific_auth_data`. The
driver always answered with `op_cont_auth`, for which such a server has no
pending authentication, hence `isc_unavailable`. Fixed; SRP now connects with no
`FB_AUTH` override. Legacy_Auth stays as a fallback.

**§4, framing — three bugs, none of them alignment padding.**

- *End of cursor* (`status = 100`) consumed the status word but not the
  `messages` word after it. Those 4 zero bytes were the "zero prefix" that made
  retaining the tail look wrong; the next reader saw opcode 0.
- *`execute2`* parsed `op_response` without reading its opcode first.
- *Blobs mid-batch.* A row's blob was fetched while the rest of the 200-row
  batch was still in flight, so the blob reply was read out of the middle of it.
  Blobs are now resolved as rows are delivered, after the batch is read.

With those fixed, retaining the tail is correct, and the reader now does exactly
that: one connection buffer, each reply parsed out by exact length, topped up
from the socket when a parse runs short.

**The BLOB "hang" was a fourth bug.** `BUFFER_LENGTH` had been raised to 65536,
and the server reads the segment length as USHORT — 0. Every `op_get_segment`
returned an empty segment and never the last one. Capped at 65535; the same cap
fixes the prepare and info buffer lengths.

| Query | Before | After | vs Python driver |
|---|---|---|---|
| `select * from PRODUTO` (174 cols) | hang | 1517 rows, 378 ms | identical |
| procedure sources (1550 text BLOBs) | `rejected with code 0` | 1550 rows, 1.6 s | identical |

Blob reads cost three round trips each (open, get, close). Lazy blob loading in
the grid is a `fb-core` concern for M2, not a driver one.

## 9. Plan and performance counters (2026-10-26)

`op_info_sql` (plan) and `op_info_database` (counters) are in `fb-wire`
(`info.rs`). `fb-probe --perf "<sql>"` runs one statement in a read-only
transaction and prints the Performance Analysis block, which meets M0 item 3:

```
PLAN (PRODUTO NATURAL)
  prepare 59.7 ms · execute 0.9 ms · fetch (1 row) 2.0 ms
  memory  current 26562592 · max 27115328 · buffers 1311
  reads 45 · writes 0 · fetches 1700 · marks 0
  PRODUTO (317)   indexed 0   non-indexed 1517
```

Checked on the ERP database: an indexed range over 501 ids reports 501 indexed reads;
an unindexable filter reports `NATURAL` and non-indexed reads equal to the
table's row count. The FB3+ explained plan comes back as the tree form.

Notes for `fb-core`:

- **The high-level `rsfbclient::Connection` is out.** It keeps the client and
  statement handles private, so nothing can ask for a statement's plan. The
  connection actor drives `RustFbClient` through the `rsfbclient-core` traits
  plus these inherent methods; `rsfbclient-core` still supplies the types.
- Counters are cumulative per attachment; a statement's cost is the difference
  of snapshots taken around execute + fetch. The snapshot itself is one round
  trip, so the editor should take them only while the Performance tab is on.
- Per-table counts are 6-byte entries (relation id, 32-bit count). The server
  widens a count past 2^31 to 8 bytes with nothing marking it; the parser
  reports that case as an error instead of misreading every entry after it.

## 5. Container matrix — still blocked

Pending: the 2.5 / 3.0 / 4.0 / 5.0 matrix. (Reading the performance block from a real
query is done — §9 — but only against FB 4.0.4.)

`docker/compose.yml` and the fixture SQL in `docker/seed/` are written and ready.

The dev machine has no container runtime — no Docker, Podman, Colima, Lima or OrbStack —
and Homebrew has no `firebird` formula. Options, cheapest first:

1. **`brew install colima docker docker-compose`** then run the `firebirdsql/firebird`
   images. Standard, scriptable, gives the whole version matrix and the same setup CI uses.
2. **Point the spike at an existing Firebird server** on the network. Fast, but only
   covers whichever version that server runs, and the spike would touch real data
   (read-only `SELECT`s, but still).
3. **Native Firebird install** from firebirdsql.org — one version per machine, no matrix.

~~Recommendation: option 1.~~ Ruled out: this host does not handle virtualization
well, so no container runtime here. The matrix moves to CI (GitHub Actions runners
have Docker), and local development uses existing servers on the network (option 2),
read-only.

---

## 6. Revised priority

The driver work is no longer a background task running parallel to the UI. Order:

1. ~~Wire reader framing rewrite~~ — done, §8.
2. **Auth plugin as a connection setting** (SRP fixed in §8; Legacy_Auth still an env var).
3. ~~`op_info_sql` (plan) and `op_info_database` (performance counters)~~ — done, §9.
4. `op_cancel`, then events, then services.

## 7. Not blocked, proceeding

- Tauri shell + 1M-row virtualized grid — synthetic rows, needs no server.
- `docker/compose.yml` for the version matrix, ready for whenever a runtime exists.
- Workspace scaffold, MIT license.
