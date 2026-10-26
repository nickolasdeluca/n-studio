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
| **NUMERIC / DECIMAL through `f64`** — *fixed, §12* | `XSqlVar::coerce` asks the server for any scaled integer as `DOUBLE`; `SqlType` has no decimal variant. `INT128`, `DECFLOAT` and the time-zone types fail with "Unsupported column type". | Plan §3: values must never touch `f64`. Every money column in an ERP database. |
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

## 10. Grid spike — Tauri 2 + virtualized grid (2026-10-26)

`spikes/grid-shell`: Tauri 2, React 19, TanStack Virtual on both axes. A Rust
cursor fetches forward 200 rows at a time as the grid scrolls and keeps every
fetched row for scroll-back; the JS side holds at most 100 pages. Runs
unattended with `FBX_BENCH=synthetic:<rows>:<cols>` or `FBX_BENCH=sql:<select>`.

| Result | Jump to random row, p50 / p95 | Scroll frame p50 / p95 | Blank frames | Shell RSS | WebKit Δ |
|---|---|---|---|---|---|
| 1k × 6 (baseline) | 19 / 29 ms | 17 / 18 ms | 0 | 62 MB | 163 MB |
| 1M × 6 | 38–44 / 51–55 ms | 17 / 18–19 ms | 0 | 337 MB¹ | ~300 MB |
| 1M × 30 | 48–55 / 59–68 ms | 17–20 / 21–24 ms | 0 | 1370 → **761 MB**² | ~400 MB |
| PRODUTO, 174 cols, live | 28 / 57 ms | 17 / 19 ms | 0 | 81 MB | ~260 MB |

"Jump" is from setting the scroll position to the first frame with every
visible row filled in. ¹ per-cell store; ² per-cell → per-page store.

**The WebView is not the risk the plan feared.** 60 fps with no blank frames
at 1M rows, jumps resolved in two or three frames, and the JS heap bounded by
the page cache however large the result. Wide results cost frame time
gradually (30 columns: p95 ~22 ms), not abruptly. Kept: the plan's stack.

**The scroll-back store is the real memory cost**, and it is on the Rust side.
One heap allocation per cell cost ~45 bytes of overhead each, 1.3 GB at
1M × 30. One text buffer plus a 4-byte end offset per cell, per 200-row page,
brings it to ~23 bytes per cell — about the size of the text itself. `fb-core`
starts from the paged layout. Beyond that the lever is policy, not layout:
IBExpert also keeps every fetched row, and a result nobody scrolls through
should not be fetched (fetch on scroll, explicit "fetch all").

**IPC is not the bottleneck.** ~0.9 ms per 200-row request (~210k rows/s at
6 columns, 22 KB of JSON per page); a live server delivers 1.5–4k rows/s on
wide tables. JSON stays.

**End to end** (the M0 exit criterion): `select * from PRODUTO` connects,
prepares, streams into the grid, and the Performance panel shows the plan —
including the subqueries behind PRODUTO's computed columns against `NCM` and
`PRODUTO_CODIGO_BARRAS` — and per-table reads that account for them.

Measurement caveats:

- macOS stops delivering frames to an occluded window. The benchmark drops
  samples that span a visibility change or a >1 s stall and counts them, but a
  window covered from launch simply stalls. Two `sql:` runs stalled that way;
  occlusion is the likeliest cause and is not proven. Run it with the window
  in front.
- The paged-store figures come from a debug build: the release build stopped
  compiling mid-session, with `rustc-LLVM ERROR: out of memory` for *any*
  optimized crate (`memchr` included) on a host with free memory and zero swap.
  Memory layout does not depend on the optimization level; IPC speed does.

## 11. Grid spike — Slint 1.18 (2026-10-26)

`spikes/grid-slint`: the same grid in Slint, to compare against §10 before
committing to Tauri. Rust virtualizes both axes and fills a fixed ring of row
slots straight from the shared `spikes/rowstore` page store; nothing crosses a
process or a serializer. `FBX_BENCH=synthetic:<rows>:<cols>`, same phases as
grid-shell; scrolling is driven by a 60 Hz timer, the way input arrives.

| Result (release) | Jump p50 / p95 | Scroll frame p50 / p95 | Frames > 20 ms (of 600) | Render p50 | RSS |
|---|---|---|---|---|---|
| FemtoVG 1M × 6 | 17 / 35 ms | 17 / 33 ms | 114 | 10.5 ms | 205 MB |
| FemtoVG 1M × 30 | 29 / 36 ms | 17 / 32 ms | 142 | 11.7 ms | 688 MB |
| Skia-OpenGL 1M × 6 | 17 / 29 ms | 17 / 32 ms | 183 | 7.9 ms | 205 MB |
| Skia-OpenGL 1M × 30 | 17 / 33 ms | 17 / 31 ms | 173 | 8.4 ms | 689 MB |

**Slint wins jumps and memory.** A jump lands in one frame (Tauri: two or
three), and with no WebKit process the total is 1/3 to 2/3 of Tauri's.

**Slint loses continuous scrolling.** 20–30% of frames miss vsync (p95 ~32 ms,
a doubled frame) where Tauri holds 18–24 ms. Render alone is 8–12 ms; the rest
is likely the whole-window redraw and per-slot model updates. Probably
tunable, not proven.

**The deciding factor is the SQL editor.** Nothing for Slint comes near
CodeMirror 6. `slint-codeeditor` is an MVP (no selection, undo or clipboard),
`slint-editor` stalled in 2023, Slint's `StyledText` is display-only (selection
is an open PR, slint-ui/slint#13985; #9560 excludes editing). `cosmic-text`
(`SyntaxEditor`, undo, Vi mode; powers COSMIC Edit) is the only viable base,
but input, IME, scrolling, completion popups, multi-cursor, folding and search
would all be ours. `parley`'s `PlainEditor` takes a single style, so no
highlighting. **Kept: Tauri 2.**

Caveat: the Skia-Metal renderer never calls the rendering notifier, so it
cannot be measured this way; the benchmark has a 5 s no-frame watchdog that
reports this instead of hanging. Measured with `SLINT_BACKEND=winit-femtovg`
and `winit-skia-opengl` (`--features skia`).

## 12. Exact numerics and Firebird 4 types (2026-10-26)

The fix needs a decimal variant in `SqlType`, which lives in `rsfbclient-core`,
so that crate is vendored too: `crates/fb-wire-core`, patched in the same way
as `fb-wire`.

- `Decimal` (in core): an `i128` mantissa and a scale of up to 38, so it holds
  every NUMERIC Firebird has, `INT128` included. Prints and parses exactly; no
  dependency. `f64` only when a caller explicitly asks for one.
- Scaled `SMALLINT` / `INTEGER` / `BIGINT` are requested as `INT64` with their
  own scale, and read as `Decimal`. `INT128` is read natively: 16 bytes,
  big-endian, confirmed with 1, −1, 256 and a 32-digit value.
- `DECFLOAT(16|34)`, `TIME` / `TIMESTAMP WITH TIME ZONE` and an untyped
  `select null` are requested as `VARCHAR(64)`: the server formats them, which
  is exact and saves implementing IEEE 754 decimal and the zone database
  client-side. `fb-core`'s value type can parse them later if the grid needs
  more than text.
- Parameters: a `Decimal` goes as a scaled `INT64` when it fits, as text
  otherwise; the server converts either exactly.

Checked on FB 4.0.4 by `fb-probe`'s new "exact numerics" block, which runs in
every mode and on every server (the FB4 cases are skipped below 4.0): ten cases,
including `0.1 + 0.2` in `NUMERIC(18,2)` reading `0.30`, the BIGINT edge
`-922337203685477.5808`, a 32-digit `NUMERIC(38,2)` and both parameter paths.
`select * from PRODUTO` (1517 rows, 174 columns) gives the same fingerprint as
the Python reference driver, now including an exact sum of every decimal
(`394641.2595`).

The coerced type code is what `Column::raw_type` reports, so DECFLOAT and the
zoned types report `VARYING`. The grid will want the declared type for
alignment and editing; that belongs in `fb-core`'s column metadata, from the
XSQLDA, not in the value.

## 5. Version matrix — done in CI (2026-10-26)

Containers cannot run on the dev host (it does not handle virtualization well),
so the matrix runs in GitHub Actions: `.github/workflows/ci.yml`. Each target
starts its service from `docker/compose.yml`, is seeded from `docker/seed/`
plus 100k `BULK_ROWS`, and is probed by `fb-probe`, which now exits non-zero on
an unseeded fixture, mangled text or any value that does not read back exactly.

| Target | Server | Handshake taken | Result | 100k rows |
|---|---|---|---|---|
| 2.5 (`jacobalberty/firebird:2.5-ss`) | 2.5.9 | pre-3.0 protocol | all checks pass | 0.62 s |
| 3.0, legacy setup (`fb30`) | 3.0.14 | Legacy_Auth, no encryption | all checks pass | 0.62 s |
| 3.0, defaults (`fb30srp`) | 3.0.14 | Srp (SHA-1), wire encrypted | all checks pass | 1.10 s |
| 4.0 | 4.0.7 | Srp256, Arc4 | all checks pass | 0.87 s |
| 5.0 | 5.0.4 | Srp256, Arc4 | all checks pass | 1.10 s |

"All checks" covers the exact-numerics block (§12), the fixture's extremes
(BIGINT and NUMERIC(18,4) limits, `9999-12-31` timestamps, BOOLEAN on 3.0+,
INT128, DECFLOAT and the zoned types on 4.0+, a NULL INT128) and the
WIN1252 / UTF8 accented-text round trip.

This also settles §1's open question: the pure-Rust driver connects to 2.5,
and the encrypted SRP path (`op_cond_accept`, `op_cont_auth`, Arc4), which the
FB 4.0.4 server used during development never takes, works on 3.0, 4.0 and
5.0. Every handshake path the driver has is now exercised on every push.
Encryption costs throughput: 100k rows take ~1.1 s encrypted against ~0.6 s
in clear on the same runner, worth knowing before blaming the grid.

Images: the official `firebirdsql/firebird` publishes major tags (`3`, `4`,
`5`) and full versions, not `3.0` / `4.0` / `5.0`; the matrix follows each
major's latest patch.

---

## 6. Revised priority

The driver work is no longer a background task running parallel to the UI. Order:

1. ~~Wire reader framing rewrite~~ — done, §8.
2. **Auth plugin as a connection setting** (SRP fixed in §8; Legacy_Auth still an env var).
3. ~~`op_info_sql` (plan) and `op_info_database` (performance counters)~~ — done, §9.
4. `op_cancel`, then events, then services.

## 7. Not blocked, proceeding

- ~~Tauri shell + 1M-row virtualized grid~~ — done, §10; Slint compared, §11.
- ~~`docker/compose.yml` for the version matrix~~ — running in CI, §5.
- Workspace scaffold, MIT license.
