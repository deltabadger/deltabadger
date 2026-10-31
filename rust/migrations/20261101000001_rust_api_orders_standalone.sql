-- PLAN-A9 O4: a standalone order (no bot_id) through MCP or the REST API. `exchange_id`/`ticker_id`: the account's
-- venue and pair as validation found them; `order_type` market|limit, `price` a limit's (as sent); `surface` mcp|rest
-- (the zone a late wash-sale refusal dates in). Written at the claim: `identity` (the venue's placement identity, KuCoin
-- family, host and key fingerprint) and `order_snapshot` (the wire order: symbol, side, kind, price, volume, quote flag).
-- `external_id`: the venue's id once placed. An answered standalone `placed` row is kept (D-ORDER-STANDALONE-LOG): an
-- API cancel by exchange id finds its route here.
ALTER TABLE rust_api_orders ADD COLUMN exchange_id INTEGER;
ALTER TABLE rust_api_orders ADD COLUMN ticker_id INTEGER;
ALTER TABLE rust_api_orders ADD COLUMN order_type TEXT NOT NULL DEFAULT 'market';
ALTER TABLE rust_api_orders ADD COLUMN price TEXT;
ALTER TABLE rust_api_orders ADD COLUMN surface TEXT;
ALTER TABLE rust_api_orders ADD COLUMN identity TEXT;
ALTER TABLE rust_api_orders ADD COLUMN order_snapshot TEXT;
ALTER TABLE rust_api_orders ADD COLUMN external_id TEXT;
CREATE INDEX index_rust_api_orders_on_log ON rust_api_orders (user_id, exchange_id, external_id);
