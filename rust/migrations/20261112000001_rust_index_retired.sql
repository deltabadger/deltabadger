-- A17-4 r18 (PLAN-A17 "Retired categories", D-INDEX-RETIRED-KEPT): an index external id the install's provider no longer
-- lists in a validated catalogue (jobs::categories on CoinGecko, the data-api indices pull). Rust-only; keyed as bots follow
-- an index (every row with that id, any source). A later validated catalogue that lists the id again deletes the row. Not
-- cleared at a takeover: it is the provider's catalogue, not a row Rails rewrites.
CREATE TABLE rust_index_retired (external_id TEXT NOT NULL, provider TEXT NOT NULL, retired_at TEXT NOT NULL, PRIMARY KEY (external_id, provider));
