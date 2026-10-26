# crates/fb-wire-core

A vendored copy of [`rsfbclient-core`](https://github.com/fernandobatels/rsfbclient)
0.27.0 (MIT, see `LICENSE`): the types, traits, charsets and `ibase` constants shared by the
`rsfbclient` backends. Wired in via `[patch.crates-io]` in the workspace
manifest, next to `crates/fb-wire`; see its `VENDORED.md` for why we carry
copies at all.

## Changes so far

- **`Decimal` and `SqlType::Decimal`** (`decimal.rs`). Exact NUMERIC / DECIMAL:
  an `i128` mantissa and a scale of up to 38. Upstream had no decimal variant,
  so the wire backend turned scaled integers into `f64`. Converts to `String`,
  to `i64` when the value is a whole number that fits, and to `f64` only on
  request; usable as a parameter (`IntoParam`).
- **Firebird 4 type codes** in `ibase.rs`: `SQL_INT128`, `SQL_DEC16`,
  `SQL_DEC34`, `SQL_TIME_TZ`, `SQL_TIMESTAMP_TZ` and their `_EX` forms.
