# REST API

The REST API lets your own scripts read and control Deltabadger — the same bots, exchanges, orders, transactions and rules as the MCP tools — over plain JSON with a bearer token.

## Token and base URL

Open **Settings → Connect → REST API**. The widget shows the base URL — your instance address followed by `/api/v1`, built from `APP_ROOT_URL` (see [Configuration](38-configuration.md)) — and your personal API token. Both copy on click. The token is created for you and does not expire.

Send it on every request:

```bash
curl -H "Authorization: Bearer $TOKEN" http://localhost:3737/api/v1/bots
```

Browser sessions are not accepted on `/api/v1`; only the bearer header counts.

**Regenerate** revokes the current token immediately and shows a new one. The confirmation warns that scripts using the old token stop working until you update them. Do this whenever you suspect the token has leaked.

**Download API docs** saves the full reference (`deltabadger-api.md`), including request and response examples and every error code.

## Permissions

Every endpoint is gated by a toggle in the same widget, grouped as **Read**, **Control** and **Trade**. Click a tool to flip it, or a group title to flip the group. All toggles start off, so a fresh token can do nothing until you switch on what your script needs. A call to a switched-off endpoint returns `403` with error code `tool_disabled`.

These toggles are independent of the MCP toggles (see [Tools and permissions](31-tools-and-permissions.md)): switching **List bots** on here does not switch it on for MCP, and the other way round. The REST **Read** group also holds the two transaction export tools that MCP files under Tax & Reporting. There is no tax-report generation and no Paper Trading over REST.

## Endpoints

All paths are under `/api/v1`. The toggle column names the switch that must be on.

| Method | Path | Toggle | Notes |
|---|---|---|---|
| GET | `/bots` | List bots | Optional `?status=` |
| GET | `/bots/:id` | Bot details | Includes metrics when available |
| POST | `/bots` | Create bot | Required: `exchange_name`, `base_asset`, `quote_asset`, `quote_amount`, `interval`; optional `label`, `start_at` |
| PATCH | `/bots/:id` | Update bots | `quote_amount`, `label`; bot must be stopped |
| POST | `/bots/:id/start` | Start bot | `409` if already running |
| POST | `/bots/:id/stop` | Stop bot | `409` if not running |
| GET | `/exchanges` | List exchanges | Your connected trading exchanges |
| GET | `/exchanges/:id/balances` | Exchange balances | Live exchange call; `502` if the exchange fails |
| GET | `/transactions` | List transactions | Bot trades; optional `?bot_id=`, `?limit=` (max 100) |
| GET | `/transactions/account` | Account transactions | Tracker transactions; optional `?exchange_id=`, `?from_date=`, `?to_date=`, `?entry_type=`, `?limit=` (max 200) |
| GET | `/transactions/export` | Export transactions CSV | Returns CSV, not JSON (see below) |
| GET | `/portfolio` | Portfolio summary | `empty: true` when you have no bots |
| GET | `/orders` | List open orders | Optional `?exchange_name=`; open orders from the app and live from exchanges |
| POST | `/orders` | Market buy / Market sell / Limit buy / Limit sell | `type` picks the toggle; needs `Idempotency-Key` (see below) |
| DELETE | `/orders/:id` | Cancel order | Numeric id = a bot's order (from `GET /orders`); an order placed through this API is cancelled by its exchange order id, with `exchange_name` |
| POST | `/rules/:id/start` | Start rule | `409` if already active |
| POST | `/rules/:id/stop` | Stop rule | `409` if not active |
| PATCH | `/rules/:id` | Update rules | `withdrawal_percentage`, `max_fee_percentage`, `min_amount`, `threshold_type`; rule must be stopped |

## Responses

Every endpoint answers with the same envelope: `{ "data": …, "error": null }` on success, `{ "data": null, "error": { "code": "…", "message": "…" } }` on failure. `code` is stable and meant for scripts; `message` may change.

The one exception is `GET /transactions/export`, which returns `text/csv` as an attachment. It is capped at 5000 rows; the headers `X-Total-Transactions`, `X-Returned-Transactions` and `X-Truncated` tell you whether to narrow the date range. Errors from this endpoint still come as JSON.

## Placing orders

`POST /orders` takes `type` (`market_buy`, `market_sell`, `limit_buy`, `limit_sell`), `exchange_name`, `base_asset`, `quote_asset`, `amount`, optional `amount_type` (`quote` or `base`) and, for limit orders, `price`. A successful placement answers `201`.

The request must carry an `Idempotency-Key` header with a value unique to this attempt (a UUID is fine); without it the call fails with `400 idempotency_key_required`. Repeating the same key with the same body returns the stored response without touching the exchange again, so a retry after a network error cannot place a second order. The same key with a different body is refused with `409 idempotency_key_reused`; a retry while the first attempt is still running gets `409 idempotency_in_progress`. Cancelling does not need a key.

## Other clients

The personal token is for scripts you write yourself. An application someone else wrote connects through the same OAuth flow as an MCP client (see [Connecting Claude](30-connecting-claude.md)) and must ask for the `api` scope when it registers — the default scope only opens the MCP server. On the **Authorize access** page you then choose what it may use over the REST API, group by group; a tool has to be both granted to the client and switched on here. Its tokens last an hour and are refreshed by the client. Your personal token has no per-client grant: your toggles are the whole answer for it.
