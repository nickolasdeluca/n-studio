# Script Executive: what is still open

The Script Executive is built (milestone M10 in `docs/PLAN.md`, a section
of its own in `README.md`). This file keeps only what is still open.

## For the owner to decide

Defaults were chosen so the work could go on; these change behaviour
people will notice.

1. **The password in `CONNECT`**: Add CONNECT writes none unless asked, and
   the run reads the keychain; IBExpert writes it in plain text. Scripts
   that already have a `PASSWORD` run as they are either way.
2. **A transaction still active when the script ends**: the panel asks, as
   IBExpert does. Always committing is what unattended runs would want; it
   is an option, not the default.
3. **Whether Run Checked applies unchecked `SET` and `CONNECT` lines**: it
   does not: a statement runs only if it is checked.

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
