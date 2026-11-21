-- Delisted coins (plan r10 §1.5, PR0): Rust-only, written beside 2.x's tables and read by nothing yet.
-- A coin's daily price keyed by the coin (its external id), not by the spelling a venue uses for it. Insert-only, as
-- historical_prices is; written after each provider answer's historical_prices write, in its own transaction.
CREATE TABLE asset_prices (
  id INTEGER PRIMARY KEY,
  external_id TEXT NOT NULL,
  currency TEXT NOT NULL,
  date TEXT NOT NULL,
  price TEXT NOT NULL,
  UNIQUE (external_id, currency, date)
);
