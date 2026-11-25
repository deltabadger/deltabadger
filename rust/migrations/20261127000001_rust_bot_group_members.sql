-- PLAN index-as-ingredient §3.3: which member came from which index group of a bot stored with `index_groups`
-- (bot_index_assets cannot say it and is 2.x's). Rust-only. `ticker_id`: incumbency is a ticker's (a venue or quote
-- change probes the new pair). `composed`: 1 when a composition here wrote the rows; only then may they stand for a
-- retired index. `weight`: within the group; a group's rows sum to 1. Cleared at a new ownership's claim.
CREATE TABLE rust_bot_group_members (
  bot_id INTEGER NOT NULL, index_external_id TEXT NOT NULL, asset_id INTEGER NOT NULL,
  ticker_id INTEGER NOT NULL,
  composed INTEGER NOT NULL,
  weight REAL NOT NULL,
  PRIMARY KEY (bot_id, index_external_id, asset_id));
