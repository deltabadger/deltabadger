-- A17-4 (PLAN-A17, D-INDEX-UNATTESTED-HELD): which provider ranked an `indices` row, under which configuration (the run's
-- engine::provider::fingerprint) and when. Rust-only; Rails never reads it and its deletes cascade. The engine composes an
-- index bot only from a row ranked here by the current provider and fingerprint (engine::index::ranked_row), fresh within
-- 49 h; a new ownership's claim (lease::claim) deletes every row.
CREATE TABLE rust_index_rankings (index_id INTEGER PRIMARY KEY REFERENCES indices(id) ON DELETE CASCADE,
  provider TEXT NOT NULL CHECK (provider IN ('data-api','coingecko')), fingerprint TEXT NOT NULL, ranked_at TEXT NOT NULL);
