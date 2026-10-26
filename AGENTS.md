# Working in this repository

Rules for anyone changing this repository, human or agent. Read before your
first commit.

## Real databases and what gets written down

Development runs against real databases — customers' and our own — as well as
the docker fixture. Whatever reaches the repository (docs, code comments,
tests, commit messages, PR descriptions, CI logs) can become public, and git
history keeps it after a later edit removes it. So, **before writing anything
learned from a real database into the repository, anonymize it**:

- **Never name the database, its company, or its server.** No database or
  file names, aliases, paths (`C:\…\X.FDB`), hostnames or IP addresses, ports
  tied to a real server, or the customer's or product's name. Describe it by
  what matters to the finding: "a production ERP database, FB 4.0.4, 1322
  tables".
- **Never copy data.** No row values, names, document numbers (CPF, CNPJ),
  addresses, emails or phone numbers, not even in a test, a fingerprint or a
  screenshot. Aggregates (row counts, timings, sizes) are fine.
- **Generic schema names are fine; distinctive ones are not.** `PRODUTO`,
  `NCM`, `CLIENTE` say nothing about whose database it is; a table or
  procedure named after a customer, a product or an in-house system does —
  replace it with a neutral one and keep the shape (column count, types).
- **Credentials only in `.env`**, which is gitignored, and never echoed into
  output that is pasted anywhere. The docker fixture's `SYSDBA`/`masterkey`
  is the one exception: it is public by design.
- If something slipped into a commit already, say so: an edit does not remove
  it from history, and rewriting pushed history needs the owner's decision.

## Real databases are read-only

Tools and tests connecting to a server named by the `FB_*` / `FIREBIRD_*`
variables use read-only transactions and never run DDL or DML. Anything that
writes belongs to the docker fixture (`docker/seed`), and tests that need it
check `FB_FIXTURE=1`.

## Commits

- No agent co-author trailer and no "generated with" footer, in commits or PR
  bodies.
- One logical change per commit; the message says why, not only what.
- `cargo build --workspace` without warnings and `cargo test` passing before
  committing. Live tests skip themselves without `FB_DB`; run them against a
  server when touching `fb-wire`, `fb-core` or `fb-meta`.
- Vendored crates (`crates/fb-wire`, `crates/fb-wire-core`) are not run
  through `cargo fmt`: they keep upstream's layout so changes stay reviewable
  and can go back upstream. Record each change in their `VENDORED.md`.

## Tooling

The project is Rust (and the UI's TypeScript). Python appears only as an
external reference in scratch work, never as part of the project.
