# Script Executive: what is still open

The Script Executive is built (milestone M10 in `docs/PLAN.md`, a section
of its own in `README.md`). This file keeps only what is still open.

## Not yet tried

- A file near the 256 MB limit run end to end: splitting one was measured
  (about 2.4 s, little beyond the text held), the run was not. The owner
  has run scripts against a real database in the app; none that large.

## Later steps

- Following `INPUT 'file'` (reported as not followed today).
- Asking, rather than refusing, when the database is changed while a
  transaction a run left is active.
- Listing more than 200,000 statements of a file, if scripts that large
  turn out to need picking from.
