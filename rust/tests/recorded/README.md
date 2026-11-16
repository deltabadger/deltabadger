# Rails' frozen answers

Recorded once from Rails at the commit tagged `rails-oracle-final` (PLAN-B11 B11-1) by `script/rust/freeze`, and from
then on the Rust tests' expected values: the tests read only these files and never call Rails.

- `<name>/`: a whole Rails build (`common::frozen`): the install template, the grids, the pages recordings.
- `<test binary>/<command>-<digest>/`: one Rails command on one input (`common::freeze::step`), keyed by the input's
  contents; only the files the command created or changed. A test that hands Rails another input fails: nothing is left
  to answer it.
- A SQLite database is stored as `<file>.sql` (rows with their rowids, `sqlite_sequence`, then indexes and triggers) and
  `<file>.pragmas.json`; the loader in `tests/common/freeze.rs` restores both.
- `rails_job_classes.txt`: every 2.x job class (tests/handover.rs).
- `MASK`: the values Rails draws fresh on every run (clock stamps, salts, ciphertexts, tokens, ids). Two freezes are
  equal once those are blanked: `script/rust/freeze --compare A B`.

The freeze writes `<repo>` for the checkout, `~` for the home directory and `host` for the mail host, so the machine
that froze them leaves no trace here. No test reads those values.
