-- A28-1, D-RAILS-UNRECORDED-REPORTED: one Rails tick that may have sent an order Rails never recorded (Rails' own KNOWN
-- GAP, action_job.rb:167-183). Rust-only. Written in a takeover's claim transaction (engine::rails_unrecorded::collect),
-- one row per source: 'ambiguous_line' (bot_activity_logs id), 'queue_job' (solid_queue_jobs id), 'failed_row'
-- (transactions id). `span_from`/`span_to`: when Rails was placing; `scope`: the pair and side a failed row recorded.
-- The report job reads the ledger the user already has and writes the bot one line; `reported_at` is set with it, once.
CREATE TABLE rust_rails_unrecorded (id INTEGER PRIMARY KEY, bot_id INTEGER NOT NULL, source TEXT NOT NULL, source_ref INTEGER NOT NULL,
  span_from TEXT NOT NULL, span_to TEXT NOT NULL, scope TEXT, collected_at TEXT NOT NULL, reported_at TEXT, outcome TEXT);
CREATE UNIQUE INDEX index_rust_rails_unrecorded_on_source ON rust_rails_unrecorded (source, source_ref);
