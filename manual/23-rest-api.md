# REST API

The REST API lets your own scripts read and control Deltabadger — the same bots, exchanges, orders, transactions, rules and tax reports as the MCP tools — over plain JSON with a bearer token. Both surfaces share one catalogue, so every tool MCP offers, REST offers too.

## Token and base URL

Open **Settings → MCP & REST API**. The **REST API** widget shows the base URL — your instance address followed by `/api/v1`, built from `APP_ROOT_URL` (see [Configuration](29-configuration.md)) — and your personal API token. Both copy on click. The token is created for you and does not expire.

Send it on every request:

```bash
curl -H "Authorization: Bearer $TOKEN" http://localhost:3737/api/v1/bots
```

Browser sessions are not accepted on `/api/v1`; only the bearer header counts.

**Regenerate** revokes the current token immediately and shows a new one. The confirmation warns that scripts using the old token stop working until you update them. Do this whenever you suspect the token has leaked.

**Download API docs** saves the full reference (`deltabadger-api.md`), including the OAuth flow, request and response examples and every error code.

## Permissions

Every endpoint is gated by a switch in the **REST API** column of **Tool permissions**, on the same page, grouped as **Read**, **Control**, **Trade** and **Tax & Reporting**. Click a switch to flip one tool, or the switch on a group row to flip the group. All REST switches start off, so a fresh token can do nothing until you switch on what your script needs. A call to a switched-off endpoint returns `403` with error code `tool_disabled`.

The REST column is independent of the MCP column (see [Tools and permissions](22-mcp-server.md#tools-and-permissions)): switching **List bots** on for REST does not switch it on for MCP, and the other way round. There is no Paper Trading over REST — its trade endpoints place real orders.

## Endpoints

All paths are under `/api/v1`. The toggle column names the switch that must be on.

### Bots

| Method | Path | Toggle | Notes |
|---|---|---|---|
| GET | `/bots` | List bots | Optional `?status=` |
| GET | `/bots/:id` | Bot details | Includes metrics when available; a basket bot also reports its members and their weights, and an index or basket bot reports any holdings that left the composition plus a pending redeploy offer |
| POST | `/bots` | Create bot / Create index bot | `type` picks the toggle: `dca` (default) or `index`. Both need `exchange_name`, `quote_asset`, `quote_amount`, `interval`. `dca` also takes `base_asset`, or `assets` for a basket of 2–20 — an array of `{symbol, allocation}` or the string `"BTC:60,ETH:40"`, weights optional and summing to 100 when given, with optional `weighting: market_cap`. `index` also takes `index` (id from `GET /indices`), `num_coins`, `allocation_flattening` |
| PATCH | `/bots/:id` | Update bots | Bot must be stopped. Any bot: `quote_amount`, `label`. Index bots: `num_coins`, `allocation_flattening`. Basket bots: `allocations` — every current member, summing to 100. Membership is not editable |
| POST | `/bots/:id/start` | Start bot | `409` if already running |
| POST | `/bots/:id/stop` | Stop bot | `409` if not running |
| POST | `/bots/:id/archive` | Archive bot | Stops the bot first |
| DELETE | `/bots/:id/archive` | Reactivate bot | Returns the bot stopped |
| DELETE | `/bots/:id` | Delete bot | Any status; the schedule is cancelled and the history hidden |
| POST | `/bots/:id/liquidations` | Sell exited holding | Body `symbol`; index and basket bots only; irreversible market sale; needs `Idempotency-Key` (see below) |
| POST | `/bots/:id/redeploy` | Answer redeploy offer | Body `accept: true\|false`; needs `Idempotency-Key` |
| GET | `/indices` | List indices | Optional `?exchange_name=` |

### Exchanges and orders

| Method | Path | Toggle | Notes |
|---|---|---|---|
| GET | `/exchanges` | List exchanges | Your connected trading exchanges |
| GET | `/exchanges/:id/balances` | Exchange balances | Live exchange call; `502` if the exchange fails |
| GET | `/orders` | List open orders | Optional `?exchange_name=`; open orders from the app and live from exchanges |
| POST | `/orders` | Market buy / Market sell / Limit buy / Limit sell | `type` picks the toggle; needs `Idempotency-Key` (see below) |
| DELETE | `/orders/:id` | Cancel order | Numeric id = a bot's order (from `GET /orders`); an order placed through this API is cancelled by its exchange order id, with `exchange_name` |

### Transactions and Tracker

| Method | Path | Toggle | Notes |
|---|---|---|---|
| GET | `/transactions` | List transactions | Bot trades; optional `?bot_id=`, `?limit=` (max 100) |
| GET | `/transactions/account` | Account transactions | Tracker transactions; optional `?exchange_id=`, `?from_date=`, `?to_date=`, `?entry_type=`, `?limit=` (max 200) |
| GET | `/transactions/export` | Export transactions CSV | Returns CSV, not JSON (see below) |
| POST | `/transactions/account/:id/transfer_link` | Mark transfer | Body `linked: true\|false`; give either side of the pair, the match is found within 14 days |
| PATCH | `/transactions/account/:id/price` | State a price | Body `price_usd` (`null` or `""` clears it); USD is the ledger's unit |
| POST | `/tracker/sync` | Sync tracker | No body; runs in the background |
| GET | `/portfolio` | Portfolio summary | `empty: true` when you have no bots |

### Rules

| Method | Path | Toggle | Notes |
|---|---|---|---|
| GET | `/rules` | List rules | Destinations are returned masked |
| POST | `/rules` | Create rule | Created stopped; `address` must already be on the exchange's withdrawal allow-list |
| PATCH | `/rules/:id` | Update rules | `withdrawal_percentage`, `max_fee_percentage`, `min_amount`, `threshold_type`; rule must be stopped |
| DELETE | `/rules/:id` | Delete rule | Rule must be stopped |
| POST | `/rules/:id/start` | Start rule | `409` if already active |
| POST | `/rules/:id/stop` | Stop rule | `409` if not active |

### Tax reports

| Method | Path | Toggle | Notes |
|---|---|---|---|
| GET | `/tax/jurisdictions` | Tax jurisdictions | Supported countries, method and currency |
| POST | `/tax/reports` | Generate tax report | `country`, `year`, optional `stablecoin_as_fiat` and `force`; answers `202`, then poll the status |
| GET | `/tax/reports/:country/:year` | Tax report status | `{ ready, state }` — `ready`, `generating` or `none`; one report runs at a time per account |
| GET | `/tax/reports/:country/:year/download` | Download tax report | Returns CSV; `404` until generated |

This is the [crypto tax report](19-crypto-tax-report.md) only; the [broker tax report](20-broker-tax-report.md) is not available over the API.

## Responses

Every endpoint answers with the same envelope: `{ "data": …, "error": null }` on success, `{ "data": null, "error": { "code": "…", "message": "…" } }` on failure. `code` is stable and meant for scripts; `message` may change.

The exceptions are `GET /transactions/export` and the tax report download, which return `text/csv` as an attachment. The export is capped at 5000 rows; the headers `X-Total-Transactions`, `X-Returned-Transactions` and `X-Truncated` tell you whether to narrow the date range. Errors from these endpoints still come as JSON.

## Placing orders

`POST /orders` takes `type` (`market_buy`, `market_sell`, `limit_buy`, `limit_sell`), `exchange_name`, `base_asset`, `quote_asset`, `amount`, optional `amount_type` (`quote` or `base`) and, for limit orders, `price`. A successful placement answers `201`.

The request must carry an `Idempotency-Key` header with a value unique to this attempt (a UUID is fine); without it the call fails with `400 idempotency_key_required`. Repeating the same key with the same body returns the stored response without touching the exchange again, so a retry after a network error cannot place a second order. The same key with a different body is refused with `409 idempotency_key_reused`; a retry while the first attempt is still running gets `409 idempotency_in_progress`. Keys are kept for 24 hours.

`POST /bots/:id/liquidations` and `POST /bots/:id/redeploy` also sell at a venue and need the same header; there the key is scoped to the bot and the action as well as the body. Cancelling does not need a key.

## Other clients

The personal token is for scripts you write yourself. An application someone else wrote connects through the same OAuth flow as an MCP client (see [MCP server](22-mcp-server.md)) and must ask for the `api` scope when it registers — the default scope only opens the MCP server. On the **Authorize access** page you then choose what it may use over the REST API, group by group; a tool has to be both granted to the client and switched on here. Its tokens last an hour and are refreshed by the client. Your personal token has no per-client grant: your switches are the whole answer for it.
