-- PLAN-A9 O3: an order sent through MCP or the REST API, waiting for the engine (D-ORDER-QUEUED). Rust-only and
-- short-lived: the web inserts it `queued` and waits; the engine takes it (`taken`), consumes it with the order's intent
-- (`sending`) and answers it (`done`, with `outcome`); a request nobody took in time is `withdrawn` by the web.
-- `supplied`: the caller's exchange_name/base_asset/quote_asset (null when left out), re-checked against the bot when
-- taken. `credential_version`: the key validation saw; a different key at take time places nothing. `send_by`: no order
-- request leaves after it.
CREATE TABLE rust_api_orders (
  id INTEGER PRIMARY KEY,
  user_id INTEGER NOT NULL,
  bot_id INTEGER,
  side TEXT NOT NULL,
  amount TEXT NOT NULL,
  amount_type TEXT NOT NULL,
  supplied TEXT NOT NULL,
  credential_version TEXT,
  state TEXT NOT NULL,
  cl_ord_id TEXT,
  outcome TEXT,
  created_at TEXT NOT NULL,
  send_by TEXT NOT NULL,
  taken_at TEXT
);
CREATE INDEX index_rust_api_orders_on_state ON rust_api_orders (state, id);
