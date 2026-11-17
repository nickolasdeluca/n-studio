# Script Executive: what is still open

The Script Executive is built (milestone M10 in `docs/PLAN.md`, a section
of its own in `README.md`). This file keeps only what is still open.

## Not yet tried

- A file near the 256 MB limit run end to end: splitting one was measured
  (about 2.4 s, little beyond the text held), the run was not. The owner
  has run scripts against a real database in the app; none that large.

Nothing else is open: following `INPUT` was built, and the owner decided
that the database stays unchangeable while a transaction is active, and
that a file lists only its first 200,000 statements.
