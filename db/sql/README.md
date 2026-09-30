# db/sql

The Rust backend (`rust/`) creates and upgrades this app's SQLite databases from these files, without Ruby.
Rails remains the schema author; `test/db/sql_artifacts_test.rb` fails when they disagree with it.

- `primary_baseline.sql` — the primary schema at the Rust floor. Frozen: never regenerate it after release.
- `migrate/<version>_<name>.sql` — for every migration newer than the baseline, the same change in SQL,
  schema AND data, proven by `test/db/sql_twins/<version>_test.rb` on a populated fixture. If a data
  change cannot be written in SQL, regenerate the baseline at that version instead (`FORCE=1 bin/rails
  db:sql:baseline`): the Rust backend then refuses older databases and asks for an upgrade with this app.
- `queue_baseline.sql`, `cache_baseline.sql`, `cable_baseline.sql` — `bin/rails db:sql:baseline`.
- `seed.sql.gz` — what `db/seeds.rb` inserts, timestamps fixed — `bin/rails db:sql:seed`.
