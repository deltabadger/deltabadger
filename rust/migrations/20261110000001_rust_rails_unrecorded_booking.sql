-- A28-2 (PLAN-A28-2, D-RAILS-UNRECORDED-RECOVERED): booking an order Rails sent but never recorded, on the bot owner's
-- confirmation. Rust-only; 2.x never reads these.
-- `offers`: the bookable venue orders a served venue's live line named (engine::rails_unrecorded::settle), `facts` the
-- order as the venue listed it (JSON). `resolutions`: one per obligation, ever: 'booked' (the offer, the row written and
-- the confirmed gross totals) or 'not_mine'. `requests`: an owner's confirmation waiting for the engine's re-check
-- (serve_bookings); `attempts`/`last_at` count transient venue failures. `notices`: once-only lines per booked row
-- ('unsettled', 'lower') and when a lowering answer was first held ('held').
CREATE TABLE rust_rails_unrecorded_offers (id INTEGER PRIMARY KEY, obligation_id INTEGER NOT NULL, external_id TEXT NOT NULL, symbol TEXT NOT NULL,
  facts TEXT NOT NULL, UNIQUE (obligation_id, external_id));
CREATE TABLE rust_rails_unrecorded_resolutions (obligation_id INTEGER PRIMARY KEY, resolution TEXT NOT NULL, offer_id INTEGER, transaction_id INTEGER,
  resolved_at TEXT NOT NULL, confirmed_base TEXT, confirmed_quote TEXT);
CREATE TABLE rust_rails_unrecorded_requests (obligation_id INTEGER PRIMARY KEY, offer_id INTEGER NOT NULL, requested_at TEXT NOT NULL,
  attempts INTEGER NOT NULL, last_at TEXT);
CREATE TABLE rust_rails_unrecorded_notices (transaction_id INTEGER NOT NULL, kind TEXT NOT NULL, at TEXT NOT NULL, PRIMARY KEY (transaction_id, kind));
