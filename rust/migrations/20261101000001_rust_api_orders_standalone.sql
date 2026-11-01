-- PLAN-A9 O4: what a standalone order (no bot_id) through MCP or the REST API carries beside its rust_api_orders row
-- (`order_id` = that row's id). A side table, not new columns: Rust migrations alter no table (B1 r4.2).
-- `exchange_id`/`ticker_id`: the account's venue and pair as validation found them; `order_type` market|limit, `price` a
-- limit's; `surface` mcp|rest (the zone a late wash-sale refusal dates in). Written at the claim: `identity` (the venue's
-- placement identity: KuCoin family, host, key fingerprint) and `order_snapshot` (the wire order: symbol, side, limit,
-- volume, quote flag). `external_id`: the venue's id once placed. A placed standalone order's two rows are kept
-- (D-ORDER-STANDALONE-LOG): an API cancel by exchange id finds its route here.
CREATE TABLE rust_api_order_standalone (
  order_id INTEGER PRIMARY KEY,
  exchange_id INTEGER NOT NULL,
  ticker_id INTEGER NOT NULL,
  order_type TEXT NOT NULL,
  price TEXT,
  surface TEXT NOT NULL,
  identity TEXT,
  order_snapshot TEXT,
  external_id TEXT
);
CREATE INDEX index_rust_api_order_standalone_on_log ON rust_api_order_standalone (exchange_id, external_id);
