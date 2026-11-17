-- B11-9 (PLAN-B11-9 §3, D-B11-9-V2-NAMES): the names Rust stored while 2.x was called Rails, renamed to v2. One
-- transaction (the migrator's): every row keeps its rowid, so the ids activity details and other tables point at stay.
-- The released files that created these tables (20261026000001, 20261108000001, 20261109000001, 20261110000001) keep the
-- old names as written; store.rs retires the old tables at this version.

-- 1. rust_withdrawal_ids: source 'rails' becomes 'v2', and its CHECK with it (rebuilt through a hop table).
CREATE TABLE rust_withdrawal_ids_b11_9 (r INTEGER PRIMARY KEY, exchange_id INTEGER NOT NULL, venue_id TEXT NOT NULL, owner TEXT NOT NULL,
  settled_at TEXT NOT NULL, source TEXT NOT NULL);
INSERT INTO rust_withdrawal_ids_b11_9 (r, exchange_id, venue_id, owner, settled_at, source)
  SELECT rowid, exchange_id, venue_id, owner, settled_at, CASE source WHEN 'rails' THEN 'v2' ELSE source END FROM rust_withdrawal_ids;
DROP TABLE rust_withdrawal_ids;
CREATE TABLE rust_withdrawal_ids (
  exchange_id INTEGER NOT NULL,
  venue_id TEXT NOT NULL,
  owner TEXT NOT NULL,
  settled_at TEXT NOT NULL,
  source TEXT NOT NULL CHECK(source IN ('rust','v2','operator'))
);
CREATE UNIQUE INDEX index_rust_withdrawal_ids_identity ON rust_withdrawal_ids (exchange_id, venue_id);
INSERT INTO rust_withdrawal_ids (rowid, exchange_id, venue_id, owner, settled_at, source)
  SELECT r, exchange_id, venue_id, owner, settled_at, source FROM rust_withdrawal_ids_b11_9;
DROP TABLE rust_withdrawal_ids_b11_9;

-- 2. The eight tables: the new one, its rows with their rowids, the old one dropped (its indexes go with it).
CREATE TABLE rust_v2_attempt_acks (
  rule_log_id INTEGER PRIMARY KEY,
  rule_id INTEGER NOT NULL,
  resolution TEXT NOT NULL,
  venue_id TEXT,
  at TEXT NOT NULL
);
INSERT INTO rust_v2_attempt_acks (rowid, rule_log_id, rule_id, resolution, venue_id, at)
  SELECT rowid, rule_log_id, rule_id, resolution, venue_id, at FROM rust_rails_attempt_acks;
DROP TABLE rust_rails_attempt_acks;

CREATE TABLE rust_v2_job_acks (
  active_job_id TEXT NOT NULL PRIMARY KEY,
  rule_id INTEGER NOT NULL,
  resolution TEXT NOT NULL,
  venue_id TEXT,
  at TEXT NOT NULL
);
INSERT INTO rust_v2_job_acks (rowid, active_job_id, rule_id, resolution, venue_id, at)
  SELECT rowid, active_job_id, rule_id, resolution, venue_id, at FROM rust_rails_job_acks;
DROP TABLE rust_rails_job_acks;

CREATE TABLE rust_v2_unrecorded (id INTEGER PRIMARY KEY, bot_id INTEGER NOT NULL, source TEXT NOT NULL, source_ref INTEGER NOT NULL,
  span_from TEXT NOT NULL, span_to TEXT NOT NULL, scope TEXT, collected_at TEXT NOT NULL, reported_at TEXT, outcome TEXT);
CREATE UNIQUE INDEX index_rust_v2_unrecorded_on_source ON rust_v2_unrecorded (source, source_ref);
INSERT INTO rust_v2_unrecorded (rowid, id, bot_id, source, source_ref, span_from, span_to, scope, collected_at, reported_at, outcome)
  SELECT rowid, id, bot_id, source, source_ref, span_from, span_to, scope, collected_at, reported_at, outcome FROM rust_rails_unrecorded;
DROP TABLE rust_rails_unrecorded;

CREATE TABLE rust_v2_unrecorded_live (obligation_id INTEGER PRIMARY KEY, stamp TEXT NOT NULL, attempts INTEGER NOT NULL,
  refused_at TEXT, last_at TEXT);
INSERT INTO rust_v2_unrecorded_live (rowid, obligation_id, stamp, attempts, refused_at, last_at)
  SELECT rowid, obligation_id, stamp, attempts, refused_at, last_at FROM rust_rails_unrecorded_live;
DROP TABLE rust_rails_unrecorded_live;

CREATE TABLE rust_v2_unrecorded_offers (id INTEGER PRIMARY KEY, obligation_id INTEGER NOT NULL, external_id TEXT NOT NULL, symbol TEXT NOT NULL,
  facts TEXT NOT NULL, UNIQUE (obligation_id, external_id));
INSERT INTO rust_v2_unrecorded_offers (rowid, id, obligation_id, external_id, symbol, facts)
  SELECT rowid, id, obligation_id, external_id, symbol, facts FROM rust_rails_unrecorded_offers;
DROP TABLE rust_rails_unrecorded_offers;

CREATE TABLE rust_v2_unrecorded_resolutions (obligation_id INTEGER PRIMARY KEY, resolution TEXT NOT NULL, offer_id INTEGER, transaction_id INTEGER,
  resolved_at TEXT NOT NULL, confirmed_base TEXT, confirmed_quote TEXT);
INSERT INTO rust_v2_unrecorded_resolutions (rowid, obligation_id, resolution, offer_id, transaction_id, resolved_at, confirmed_base, confirmed_quote)
  SELECT rowid, obligation_id, resolution, offer_id, transaction_id, resolved_at, confirmed_base, confirmed_quote FROM rust_rails_unrecorded_resolutions;
DROP TABLE rust_rails_unrecorded_resolutions;

CREATE TABLE rust_v2_unrecorded_requests (obligation_id INTEGER PRIMARY KEY, offer_id INTEGER NOT NULL, requested_at TEXT NOT NULL,
  attempts INTEGER NOT NULL, last_at TEXT);
INSERT INTO rust_v2_unrecorded_requests (rowid, obligation_id, offer_id, requested_at, attempts, last_at)
  SELECT rowid, obligation_id, offer_id, requested_at, attempts, last_at FROM rust_rails_unrecorded_requests;
DROP TABLE rust_rails_unrecorded_requests;

CREATE TABLE rust_v2_unrecorded_notices (transaction_id INTEGER NOT NULL, kind TEXT NOT NULL, at TEXT NOT NULL, PRIMARY KEY (transaction_id, kind));
INSERT INTO rust_v2_unrecorded_notices (rowid, transaction_id, kind, at)
  SELECT rowid, transaction_id, kind, at FROM rust_rails_unrecorded_notices;
DROP TABLE rust_rails_unrecorded_notices;

-- 3. Rule evaluations imported at a takeover: rails:<active_job_id> becomes v2:<active_job_id> (exact, case-sensitive
-- prefix; no v2: source existed before, so UNIQUE (rule_id, source) cannot collide).
UPDATE rust_rule_evaluations SET source = 'v2:' || substr(source, 7) WHERE substr(source, 1, 6) = 'rails:';

-- 4. app_configs keys (updated_at untouched; an existing new key fails the migration on the unique key index).
UPDATE app_configs SET key = 'rust_v2_wakes' WHERE key = 'rust_rails_wakes';
UPDATE app_configs SET key = 'rust_v2_queue_done' WHERE key = 'rust_rails_queue_done';
UPDATE app_configs SET key = 'rust_v2_unrecorded.live_cursor' WHERE key = 'rust_rails_unrecorded.live_cursor';
UPDATE app_configs SET key = 'rust_job.v2_unrecorded_report' WHERE key = 'rust_job.rails_unrecorded_report';

-- 5. A job's 2.x baseline: rails_at becomes v2_at as the same JSON value (`->` keeps its type: a null, a boolean or a
-- number stays one); a row that is not a JSON object is untouched.
UPDATE app_configs SET value = json_set(json_remove(value, '$.rails_at'), '$.v2_at', json(value -> '$.rails_at'))
  WHERE substr(key, 1, 9) = 'rust_job.' AND json_valid(value) AND json_type(value) = 'object' AND json_type(value, '$.rails_at') IS NOT NULL;

-- 6. Bots: a signal call imported at a takeover (rails:<job> becomes v2:<job>, order and every other field kept), and the
-- placement marker Rust invented for a 2.x job (rails-job: becomes v2-job:). Only matching rows change; updated_at
-- untouched. A bot whose call list holds a non-object is left as it is (the engine reports it unreadable).
UPDATE bots SET transient_data = json_set(transient_data, '$.rust_signal_calls', (
  SELECT json_group_array(json(CASE WHEN json_type(e.value, '$.source') = 'text'
                                     AND substr(json_extract(e.value, '$.source'), 1, 6) = 'rails:'
                                    THEN json_set(e.value, '$.source', 'v2:' || substr(json_extract(e.value, '$.source'), 7))
                                    ELSE e.value END) ORDER BY e.key)
  FROM json_each(bots.transient_data, '$.rust_signal_calls') e))
WHERE json_valid(transient_data) AND json_type(transient_data, '$.rust_signal_calls') = 'array'
  AND NOT EXISTS (SELECT 1 FROM json_each(bots.transient_data, '$.rust_signal_calls') e WHERE e.type <> 'object')
  AND EXISTS (SELECT 1 FROM json_each(bots.transient_data, '$.rust_signal_calls') e
              WHERE json_type(e.value, '$.source') = 'text' AND substr(json_extract(e.value, '$.source'), 1, 6) = 'rails:');
UPDATE bots SET transient_data = json_set(transient_data, '$.signal_placement_unresolved.id',
    'v2-job:' || substr(json_extract(transient_data, '$.signal_placement_unresolved.id'), 11))
  WHERE json_valid(transient_data) AND json_type(transient_data, '$.signal_placement_unresolved.id') = 'text'
    AND substr(json_extract(transient_data, '$.signal_placement_unresolved.id'), 1, 10) = 'rails-job:';

-- 7. Activity: the ten event kinds (rails_order_* becomes v2_order_*), then the marker ids of the signal lines. A full scan
-- (no event index).
UPDATE bot_activity_logs SET event = 'v2_order_' || substr(event, 13)
  WHERE event IN ('rails_order_candidates', 'rails_order_none_seen', 'rails_order_unchecked', 'rails_order_open', 'rails_order_listed',
                  'rails_order_recovered', 'rails_order_not_added', 'rails_order_not_mine', 'rails_order_unsettled', 'rails_order_settled_lower');
UPDATE bot_activity_logs SET details = json_set(details, '$.marker_id', 'v2-job:' || substr(json_extract(details, '$.marker_id'), 11))
  WHERE event IN ('upgrade_signal_unresolved', 'signal_placement_resolved') AND json_valid(details)
    AND json_type(details, '$.marker_id') = 'text' AND substr(json_extract(details, '$.marker_id'), 1, 10) = 'rails-job:';
