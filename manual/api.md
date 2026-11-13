# Deltabadger REST API

The REST API mirrors the MCP server's read / control / trade coverage. Both
surfaces share auth (OAuth 2.1) and business logic, so changes to one are
reflected by the other.

- **Base URL:** `https://{your-subdomain}.deltabadger.com/api/v1`
- **Format:** `application/json` (with two documented exceptions: the CSV downloads)
- **Auth:** OAuth 2.1 bearer token with `api` scope
- **State changes:** `POST /api/v1/orders`, `/bots/:id/liquidations` and
  `/bots/:id/redeploy` require an `Idempotency-Key` header
- **Real money:** REST has no Paper Trading. Its trade endpoints always place
  real orders.

The REST API is intentionally OAuth-only. Web-session cookies are not honored
on `/api/v1/*` — a signed-in browser session without a bearer header is
rejected as if unauthenticated.

---

## 1. Auth

There are two paths to a valid bearer token. Most users only need the first.

### 1a. First-party / personal scripts (the simple path)

Open **Settings → REST API**. Your personal API token is created for you, but
the page never shows it: it reads `[redacted]`. Click **Regenerate** to get a
token you can copy. It is shown once, in that answer only, so store it right
away. The personal token carries the `api` scope and does not expire.

Use it in your script:

```bash
TOKEN='paste-from-settings'
curl -H "Authorization: Bearer $TOKEN" \
     https://your.deltabadger.com/api/v1/bots
```

**Regenerate** also revokes every earlier personal token immediately. Click it
again whenever you lose the token or suspect it has leaked, then update your
script with the new value.

Tool toggles still apply: by default the personal token can do nothing.
Flip the toggles for the endpoints your script needs (`list_bots`,
`market_buy`, etc.) on the same Settings page before calling them.

### 1b. Third-party clients (Claude Desktop, MCP plugins, custom OAuth integrations)

When you're delegating access to *someone else's* application — anything
that isn't a script you wrote yourself — use the standard OAuth 2.1
authorization-code-with-PKCE flow plus Dynamic Client Registration. The
client app drives the dance; you only see the consent screen.

### Scopes

| Scope | Grants access to |
|---|---|
| `mcp` | The MCP endpoint at `/mcp` |
| `api` | The REST endpoints under `/api/v1` |

`mcp` is the default scope (omit `scope` and you get `mcp` only). Request
`api` explicitly — or both — at registration time.

A token can carry both scopes; they're orthogonal. A token with only `mcp`
is rejected at `/api/v1/*` with `403 insufficient_scope`. A token with only
`api` is rejected at `/mcp` with HTTP `401`, JSON-RPC error `-32000`
("Access token missing required scope") and
`WWW-Authenticate: Bearer error="invalid_token"`.

### Dynamic client registration

```bash
curl -X POST https://{your-subdomain}.deltabadger.com/oauth/register \
  -H 'Content-Type: application/json' \
  -d '{
    "client_name": "My Script",
    "redirect_uris": ["http://localhost:8765/callback"],
    "scope": "api"
  }'
```

Response:

```json
{
  "client_id": "example-client-id",
  "client_name": "My Script",
  "redirect_uris": ["http://localhost:8765/callback"],
  "registration_access_token": "...",
  "token_endpoint_auth_method": "none",
  "grant_types": ["authorization_code"],
  "response_types": ["code"],
  "scope": "api"
}
```

Scope rules:

- Omit `scope` → defaults to `mcp` (preserves legacy behavior).
- Pass `"api"` → REST-only.
- Pass `"api mcp"` or `"mcp api"` → both (order doesn't matter; whitespace
  is normalized).
- Unknown scope tokens (e.g. `"admin"`) → `400 invalid_client_metadata`.

### Authorization code + PKCE (OAuth 2.1)

After DCR, run the standard authorization-code-with-PKCE flow:

1. **Authorize** — direct the user to:

   ```
   GET /oauth/authorize
     ?client_id={client_id}
     &redirect_uri={redirect_uri}
     &response_type=code
     &scope=api
     &code_challenge={S256(verifier)}
     &code_challenge_method=S256
   ```

2. **Token exchange** — POST the returned `code` to `/oauth/token`:

   ```bash
   curl -X POST https://{your-subdomain}.deltabadger.com/oauth/token \
     -d 'grant_type=authorization_code' \
     -d 'client_id={client_id}' \
     -d 'redirect_uri={redirect_uri}' \
     -d 'code={code}' \
     -d 'code_verifier={verifier}'
   ```

   Returns `{ "access_token": "...", "token_type": "Bearer", "expires_in": 3600, ... }`.

3. **Call the API** — include `Authorization: Bearer {access_token}` on every
   request.

Access tokens expire after 1 hour; use the returned `refresh_token` to obtain
a new one.

### Revoking a token

`POST /oauth/revoke` (RFC 7009) takes `token` (and optionally
`token_type_hint`) plus the client's `client_id`. It answers `200 {}`, also
for a token that is unknown or already dead. A token that belongs to another
client is refused with `403 unauthorized_client`.

### Introspecting a token

`POST /oauth/introspect` (RFC 7662) takes `token` (and optionally
`token_type_hint`). The caller authenticates as a client, the same way as at
`/oauth/token`, or with `Authorization: Bearer {access_token}` of the same
client. A live token of the caller's own client answers
`{ "active": true, "scope": ..., "client_id": ..., "iat": ..., "token_type": "Bearer", "exp": ... }`;
any other token answers `{ "active": false }`. A bearer token cannot
introspect itself or a token of another client (`401 invalid_token`).

`GET /oauth/token/info` with `Authorization: Bearer {access_token}` describes
that token: `resource_owner_id`, `scope`, `expires_in` (seconds left),
`application.uid` and `created_at`. A token that is unknown, expired or
revoked answers `401 invalid_token`.

### Discovery

- `GET /.well-known/oauth-authorization-server` (RFC 8414) — advertises
  `scopes_supported: ["mcp", "api"]`, endpoints, supported grant types,
  code challenge method, etc.
- `GET /.well-known/oauth-protected-resource` (RFC 9728) — points to the
  protected resource and the authorization servers.

---

## 2. Tool toggles

Every REST endpoint is gated by a per-user, per-tool toggle. **All REST
toggles default to off.** A request with a valid `:api` token whose user has
not enabled the matching toggle returns:

```http
HTTP/1.1 403 Forbidden
Content-Type: application/json

{
  "data": null,
  "error": { "code": "tool_disabled", "message": "Tool 'list_bots' is disabled for this user." }
}
```

`tool_disabled` covers two separate refusals: the tool is switched off for your
account, **or** it is not granted to the client whose token you are using. A
tool has to pass both. The account-wide toggles are on **Settings → REST API**
(and **Settings → MCP Server** for MCP); each connected client has its own
switches on its card under **Connected clients**.

A newly connected client follows your account-wide toggles: a tool you switch
on later reaches it too. Once you change that client's own switches for a
surface, it keeps the list you set there, and a tool you switch on later
reaches it only when you grant it on the client's card as well. The personal
API token is exempt: it is yours, not a third party's, so your account-wide
toggles are the whole answer for it.

Toggles are grouped (`read`, `control`, `trade`, `tax`); each row is
independent. REST toggles are **isolated from MCP** — enabling `list_bots` for
REST does not affect MCP permissions and vice versa.

Tool names are the MCP names exactly; the Tool column of the endpoint table
below lists every one. The two surfaces share one catalogue, so every tool MCP
offers, REST offers too.

---

## 3. Response envelope

All endpoints return JSON with a stable envelope:

**Success:**

```json
{ "data": { ... }, "error": null }
```

**Error:**

```json
{ "data": null, "error": { "code": "...", "message": "..." } }
```

`code` is a short machine-readable identifier; `message` is human-readable
copy that may change.

**Exceptions:** `GET /api/v1/transactions/export` and
`GET /api/v1/tax/reports/:country/:year/download` return `text/csv` directly
on success. See section 6.

A few answers come before any endpoint and carry no envelope: a path that is
not routed (`404`), a body that cannot be parsed or a path that is not valid
UTF-8 (`400`), a body too large (`413`) or too slow to arrive (`408`), and an
unexpected error (`500`). Branch on `Content-Type` before reading the
envelope.

### HTTP status mapping

Errors set both an HTTP status and an envelope `error.code`. This table lists
every code an endpoint can answer with:

| Status | Error codes |
|---|---|
| 202 | (success) work accepted: a tax report, a tracker sync, a liquidation, a redeploy answer, an unrecorded order named as the bot's |
| 400 | `idempotency_key_required` |
| 401 | `missing_token`, `invalid_token`, `token_revoked`, `token_expired`, `user_not_found` |
| 403 | `tool_disabled`, `insufficient_scope`, `api_key_missing`, `withdrawal_key_missing` |
| 404 | `bot_not_found`, `rule_not_found`, `exchange_not_found`, `pair_not_found`, `asset_not_found`, `holding_not_held`, `transaction_not_found`, `no_transfer_candidate`, `index_not_found`, `quote_asset_not_found`, `no_transactions`, `report_not_found`, `missing`, `order_not_listed` |
| 409 | `bot_already_running`, `bot_not_running`, `bot_running`, `bot_archived`, `bot_not_archived`, `write_refused`, `rule_already_active`, `rule_not_active`, `rule_active`, `rule_exists`, `rule_changed`, `exchange_changed`, `credential_changed`, `withdrawal_unconfirmed`, `market_closed`, `ticker_not_tradable`, `wash_sale_locked`, `venue_busy`, `placement_unresolved`, `api_key_pending`, `quote_amount_limit_reached`, `already_linked`, `not_linked`, `ambiguous_transfer_candidate`, `venue_valued`, `price_pending`, `idempotency_in_progress`, `idempotency_key_reused`, `report_ready`, `report_generating`, `already_checking`, `already_decided` |
| 422 | `missing_required_parameter`, `invalid_interval`, `invalid_allocation`, `invalid_date`, `invalid_entry_type`, `invalid_order_type`, `invalid_order_id`, `invalid_amount_type`, `no_updates_provided`, `exchange_name_required`, `bot_invalid`, `bot_save_failed`, `bot_start_failed`, `bot_archive_failed`, `bot_unarchive_failed`, `bot_delete_failed`, `invalid_start_fresh`, `wash_sale_question_owed`, `unknown_country`, `invalid_year`, `invalid_flag`, `invalid_carried_costs`, `carried_costs_pl_only`, `report_refused`, `market_data_not_configured`, `no_reading_keys`, `linked_required`, `not_transferable`, `export_too_large`, `price_usd_required`, `invalid_price`, `invalid_number`, `invalid_threshold_type`, `address_not_listed`, `exchange_retired`, `withdrawal_unsupported`, `withdrawal_disabled`, `invalid_bot_type`, `not_composition_bot`, `accept_required`, `invalid_basket`, `invalid_weighting`, `allocations_unbalanced`, `market_cap_unavailable`, `unsupported_setting`, `asset_not_in_basket`, `invalid_allocations`, `ambiguous_basket_asset`, `holding_ambiguous`, `bot_not_orderable`, `bot_pair_mismatch`, `below_minimum_amount`, `bot_limit_orders_unsupported`, `invalid_answer`, and the bot split's refusals: `missing`, `none`, `unavailable`, `external_sales`, `unpriced_sales`, `nothing_to_buy`, `busy`, `write_refused`, `proceeds_unanswered` |
| 500 | (no envelope) an unexpected error, such as an exchange that did not answer; retry later |
| 501 | `not_ported` — this build does not serve the request yet (see "Not served yet" below) |
| 502 | `order_failed`, `cancel_failed`, `balances_fetch_failed`, `bot_stop_failed`, `placement_ambiguous`, `exchange_unavailable` |

---

## 4. Endpoint table

All paths are under `/api/v1`. The "Tool" column names the REST toggle that
must be enabled for the call to succeed (in addition to a valid `:api`
token).

| Method | Path | Tool | Notes |
|---|---|---|---|
| GET | `/bots` | `list_bots` | Optional `?status=` filter |
| GET | `/bots/:id` | `get_bot_details` | Includes metrics if available; composition bots also report `holdings` and `exited_holdings` (holding keys) and `redeploy_offer` (quote amount as a string); a multi-asset bot reports its members as `pair` (`BTC+ETH/USD`) plus `allocations` (`{key: weight}`; raw weights as set, which may not sum to 1 until normalised) and `allocation_assets` (`[{key, asset_id, symbol, name, weight}]`). A key is the asset's symbol, or — where two holdings or members share a symbol — the symbol with the asset id appended (`POR#12`). A bot that has them also reports `unrecorded_orders`: the orders from before the update that may have gone through without being recorded: `{carry, obligations, more}`, `carry` the bot's carry toward its next order now (a quote amount as a string), each obligation `{obligation, state, from, to, offers, transaction_id, refused}` — `state` is `open`, `checking`, `booked` or `not_mine`; `offers` are `[{offer, order, carry_after}]` (`carry_after`, the carry with that order added, only while `open`); `transaction_id` is the added order's row once `booked`; `refused` is `{reason, message, at}` when the last try was refused. Open obligations come first, at most 20 in all; `more` says some are left out |
| POST | `/bots` | `create_bot`, `create_index_bot`, `create_signal_bot` | 201 on success. `type`: `dca` (default, `create_bot`), `index` (`create_index_bot`) or `signal` (`create_signal_bot`); any other is 422 `invalid_bot_type`. `dca`: `exchange_name`, `quote_asset`, `quote_amount`, `interval`, plus either `base_asset` or `assets` — an array of `{symbol, allocation}` or the string `"BTC:60,ETH:40"`, 1-100 entries, weights optional (equal split) and summing to 100 when given — with optional `weighting: market_cap`; or `base_asset` and `second_base_asset` with optional `allocation` (the first asset's percent, default 50). `index`: `exchange_name`, `quote_asset`, `quote_amount`, `interval`, plus `index` (id from `/indices`, must be available on that exchange), `num_coins`, `allocation_flattening`. Both take optional `label` and `start_at` (a starting time, read in your account's time zone unless it carries an offset). `signal`: `exchange_name`, `base_asset`, `quote_asset`, optional `label` — a bot with no schedule that trades one pair when told to, created running and without webhook rules |
| PATCH | `/bots/:id` | `update_bot_settings` | Bot must be stopped. The fields of the bot's settings page, under the page's names and with its choices — the `update_bot_settings` MCP tool lists every one with its units and the bot types that have it. Omitted fields stay as they are; `null` clears `sell_amount`, `sell_quote_amount`, `rebalance_threshold` and `start_at`. Percent fields are in percent (`limit_order_pcnt_distance`, `price_drop_limit`, `rebalance_threshold`). Basket `allocations` name any members, as `{"BTC": 70}` or the string `"BTC:70,ETH:30"`, each by its key, its asset id, or a symbol no other member shares; members not named keep their weight, and a basket whose weights do not sum to 100 saves but cannot start |
| POST | `/bots/:id/start` | `start_bot` | Optional `start_fresh`: omitted or `true` resets the schedule and can place an order immediately, unless a future starting time applies; `false` keeps the schedule but can invest amounts missed while stopped. Any other value is 422 `invalid_start_fresh`. 422 `wash_sale_question_owed` for a bot that can sell until the wash-sale question is answered. 409 if already running |
| POST | `/bots/:id/stop` | `stop_bot` | 409 if not running |
| DELETE | `/bots/:id` | `delete_bot` | Any status; soft delete, and a running bot's schedule is cancelled |
| POST | `/bots/:id/archive` | `archive_bot` | Stops the bot first |
| DELETE | `/bots/:id/archive` | `unarchive_bot` | Returns the bot stopped |
| POST | `/bots/split` | `split_bots` | Body `bot_ids` (required, basket or index bots) and optional `keep_proceeds_for` (bot ids). Each bot becomes one stopped bot per member it still trades on its exchange, buying that asset with its share of the amount at the same interval and keeping its history; the split bots are deleted. 200 with `data.bots` (`[{id, label, asset_id}]`) and `data.source_ids`. 422 on a refusal, `error.code` one of `missing`, `none`, `unavailable`, `external_sales`, `unpriced_sales`, `nothing_to_buy`, `busy`, `write_refused`, `proceeds_unanswered`, with `error.details.offers` (`[{bot_id, offer}]`, `offer` a quote amount as a string) listing the bots whose money from past sales still waits for an answer: answer with `/bots/:id/redeploy`, or name the bot in `keep_proceeds_for` |
| POST | `/bots/:id/liquidations` | `liquidate_exited_asset` | Body `symbol`, or `symbol[]` repeated to close several positions in one call — a holding key, an asset id, or a symbol no other holding shares; **Requires `Idempotency-Key`** (see section 5); 202; index/basket bots only; irreversible market sale |
| POST | `/bots/:id/redeploy` | `answer_redeploy_offer` | Body `accept: true\|false` (required); **Requires `Idempotency-Key`**; 202, `data.offer` is what would be redeployed |
| POST | `/bots/:id/unrecorded_orders` | `answer_unrecorded_order` | Answers an obligation from `GET /bots/:id`'s `unrecorded_orders`: body `obligation` and either `offer` (the order that was this bot's) or `none: true`, integers in a JSON body (digits in a form body). `offer`: 202, `data.state` is `checking`; the exchange is asked about the order and it is added to the bot's history, which counts toward the bot's plan — `GET /bots/:id` shows `booked` or why it was refused. `none`: 200, `not_mine`. 404 `order_not_listed`, 409 `already_checking` or `already_decided` (`error.details` has the `state` and the `transaction_id`), 422 `invalid_answer`. A repeated answer never adds the order twice, so no `Idempotency-Key` is needed |
| GET | `/exchanges` | `list_exchanges` | Lists user trading exchanges |
| GET | `/exchanges/:id/balances` | `get_exchange_balances` | Live exchange call; 502 on upstream failure. Served for Alpaca and Interactive Brokers; other exchanges answer `501 not_ported` |
| GET | `/transactions` | `list_transactions` | Optional `?bot_id=`, `?limit=` (max 100) |
| GET | `/transactions/account` | `list_account_transactions` | Optional `?exchange_id=`, `?from_date=`, `?to_date=`, `?entry_type=`, `?limit=` (max 200). Each row carries `linked`, `linked_transaction_id`, `stated_price_usd` and `venue_valued` |
| GET | `/transactions/export` | `export_transactions_csv` | **CSV** (see section 6) |
| POST | `/transactions/account/:id/transfer_link` | `set_transfer_link` | Body `linked: true\|false` (required); give either side of the pair, the match is found within 14 days |
| PATCH | `/transactions/account/:id/price` | `set_transaction_price` | Body `price_usd` (key required; `null` or `""` clears it); USD is the ledger's unit. A row whose own daily price can still come (its UTC day is not over) refuses a price with `price_pending`; an Alpaca crypto fee priced at the trades it was charged on (the day before its date) never waits |
| POST | `/tracker/sync` | `sync_tracker` | 202; body-less. `501 not_ported` while you hold an Interactive Brokers key: its history is not synced yet |
| GET | `/portfolio` | `get_portfolio_summary` | Returns `empty: true` for users with no bots. `501 not_ported` while a bot on an exchange other than Alpaca needs a live price |
| GET | `/indices` | `list_indices` | Optional `?exchange_name=` |
| GET | `/orders` | `list_open_orders` | Optional `?exchange_name=`. Lists your bots' open orders on every exchange; orders placed outside your bots are read live from Alpaca only. `unavailable` names an Alpaca account that could not be asked, and, as `not read live on this build`, every other exchange you hold a correct trading key on (and the one `exchange_name` names): only your bots' orders there are listed |
| POST | `/orders` | `market_buy`, `market_sell`, `limit_buy`, `limit_sell` | `type` picks the tool; **Requires `Idempotency-Key`** (see section 5). Interactive Brokers answers `501 not_ported`, and the key stays free |
| DELETE | `/orders/:id` | `cancel_order` | A numeric id is first looked up among the open orders Deltabadger tracks (from `GET /orders`). Any other id is taken as the exchange's own order id, and `exchange_name` is then required. Interactive Brokers answers `501 not_ported` |
| GET | `/rules` | `list_rules` | Withdrawal rules; destinations are masked; includes `max_interval` and `unresolved_withdrawal` |
| POST | `/rules` | `create_rule` | 201, stored as `created`; requires a listed destination and a valid withdrawal key |
| DELETE | `/rules/:id` | `delete_rule` | Rule must be created or stopped, with no unconfirmed withdrawal |
| POST | `/rules/:id/start` | `start_rule` | 409 if already active |
| POST | `/rules/:id/stop` | `stop_rule` | 409 if not active |
| PATCH | `/rules/:id` | `update_rule_settings` | Accepts `withdrawal_percentage`, `max_fee_percentage`, `min_amount`, `threshold_type`, `max_interval`, and destination fields; rule must be created or stopped |
| GET | `/tax/jurisdictions` | `list_tax_jurisdictions` | Supported countries, method, currency |
| POST | `/tax/reports` | `generate_tax_report` | `country` (any case), `year`, optional `force` to replace an existing report, optional `pl_carried_costs` (PL only: costs carried from earlier years, an amount in PLN with at most 2 decimals; `null` or `""` clears it). `stablecoin_as_fiat` is accepted (`true` or `false`) and changes nothing on this build. **202** — poll status; crypto scope only. Some countries cover only certain years, and an install whose market data comes from CoinGecko cannot generate reports yet: both answer `501 not_ported` (see below) |
| GET | `/tax/reports/:country/:year` | `get_tax_report_status` | `{ country, year, ready, state }`; `state` is `ready`, `generating`, `refused` (with `reason` and `symbols`: the report cannot be made for those assets) or `none`; `generating` is per account (one report runs at a time) |
| GET | `/tax/reports/:country/:year/download` | `download_tax_report` | **CSV** (see section 6); 404 until generated, 422 `report_refused` for a refused report |

Withdrawal rules use the same validation on REST, MCP and the web. Create takes `exchange_name`, `asset` and `address`, with optional `network`, `address_tag`, `address_name` (Kraken's listed key), `withdrawal_percentage`, `threshold_type`, `max_fee_percentage`, `min_amount` and `max_interval`. Updates accept those settings and destination fields; include `address` when changing a destination. `max_interval` is a positive integer (days), or `null` to remove the deadline. Destinations are checked against the exchange's current list on every create or update. An explicitly selected threshold is preserved; the default is `fee_percentage`.

Withdrawal rules run on Kraken, Binance, Gemini and MEXC. Other exchanges answer `422 withdrawal_unsupported`. A rule with an unconfirmed withdrawal returns `409 withdrawal_unconfirmed` on create, update, start or delete. Stop is still allowed for an active rule and preserves the unconfirmed withdrawal. List returns `unresolved_withdrawal: null` or an object with `state`, `reason`, `since` and `candidates`.

### Not served yet

This build answers `501 not_ported` where it cannot serve a request yet. The
message says what is missing. Beyond the rows above:

- **Tax report years.** Poland covers 2019 onward, Greece up to 2026, Italy
  2023–2026, Ireland 2013–2026, Sweden, the United Kingdom and Switzerland
  2020–2026, France 2019–2026, the Netherlands 2021–2026. The other countries
  cover any year. A year outside 2009–2100 is `422 invalid_year` everywhere.
- **Bots.** A bot this build cannot change answers `409 write_refused` with
  the reason. A signal bot on an exchange it does not trade, and some creates
  and splits, answer `501 not_ported`.
- **Read limits.** Bot details, balances, open orders and the portfolio
  summary first check the size of your whole account and of the market
  catalogue, whatever the request asks for. They answer `501 not_ported`
  ("Read unavailable: request data exceeds read limits") when there are more
  than 100 bots, 100 API keys or 100 exchanges; more than 10,000 bot
  transactions, index bot members, wash-sale locks or account transactions;
  more than 20,000 rows in any one catalogue table (assets, tickers, indices,
  exchange listings); a stored value over 16 KiB, or over 4 MiB in all. Open
  orders also stop at 100 tracked orders and 50 read live from an exchange.
  The same reads answer `501` when the exchange's keys change while they run;
  ask again.

---

## 5. Idempotency — POST /orders, /bots/:id/liquidations, /bots/:id/redeploy

These three endpoints can place orders at a venue, and they are the only ones that require
idempotency. Cancellation (`DELETE /api/v1/orders/:id`) is intentionally
**not** idempotency-wrapped — cancelling an already-cancelled order is a
benign no-op at the exchange level.

On `/bots/:id/liquidations` and `/bots/:id/redeploy` the key is scoped to the bot and the
action as well as the body, so one key reused against a second bot — or against the other
action — is `409 idempotency_key_reused`, never a replay of the first answer.

### Request

`POST /api/v1/orders` dispatches on a required `type` param:

- `market_buy` — spend `amount` in `quote_asset` (default) or buy `amount`
  of `base_asset` if `amount_type=base`.
- `market_sell` — sell `amount` of `base_asset` (default) or receive
  `amount` in `quote_asset` if `amount_type=quote`.
- `limit_buy` / `limit_sell` — same shape plus a required `price`.

**Through a bot.** Add `bot_id` (a running signal bot) to a `market_buy` or `market_sell` and the
order is recorded on that bot — a row on its page, confirmed, charted and taxed like any of its
orders — instead of going to the exchange and belonging to nothing. The pair comes from the bot,
so `exchange_name`, `base_asset` and `quote_asset` become optional and must agree with it when
sent. The `201` carries `bot_id`, `transaction_id` and `order_id` (`transaction_id` is null in
the rare case the exchange accepted the order but the row could not be written). Stopping the
bot refuses further orders. `amount_type` is sent to the exchange as given where the exchange takes
it; a sell is always placed in base, and a buy is converted at the current price where the exchange
takes only one denomination. Errors specific to this path: `bot_not_found` (404);
`invalid_number`, `invalid_amount_type`, `bot_not_orderable`, `bot_pair_mismatch`,
`below_minimum_amount`, `bot_limit_orders_unsupported` (422); `bot_not_running`,
`placement_unresolved`, `api_key_pending`, `market_closed`, `wash_sale_locked` (409);
`exchange_unavailable`, `order_failed`, `placement_ambiguous` (502). `placement_ambiguous` means
the order may have been placed: resend it with the same `Idempotency-Key` to get the stored
answer, never with a new one before checking the exchange. Limit orders cannot be placed through
a bot.

Required headers and params:

```http
POST /api/v1/orders
Authorization: Bearer {api_token}
Idempotency-Key: {opaque-client-string}
Content-Type: application/json

{
  "type": "market_buy",
  "exchange_name": "Binance",
  "base_asset": "BTC",
  "quote_asset": "USD",
  "amount": 100
}
```

Successful placement → `201 Created` with the standard envelope. The
exchange's order identifier (when available) is in `data.upstream`.

### Idempotency-Key rules

- The header is **required**. Missing or blank → `400 idempotency_key_required`.
  Per-tool gates (e.g. `market_buy` toggle) and `invalid_order_type` checks
  run *before* the key is consumed, so a disabled-tool or typo'd-type
  request will not pollute your idempotency table.
- Keys are opaque to the server. Use any unique-per-attempt string (UUIDv4
  is fine).
- **Replay** — same key + same request body fingerprint → returns the
  stored response byte-for-byte; the exchange is **not** called again.
  Failed responses (e.g. `502 order_failed`) are stored and replayed too:
  an exchange-rejected order is a definitive outcome. On `POST /orders` a
  `501 not_ported` answer is not stored: the key stays free. On
  `/bots/:id/liquidations` and `/bots/:id/redeploy` it is stored and replayed
  like any other answer.
- **Conflicts:**
  - Same key, request **in progress** (concurrent retry) → `409 idempotency_in_progress`. We do not block-and-wait; retry shortly.
  - Same key, request **completed**, **different body** → `409 idempotency_key_reused`. To retry with a different payload, mint a new key.
  - Same key, request **cut off by a restart** (the app stopped before answering) → answered at the next start. If it provably placed nothing, the key is free again: resend the same request. Otherwise the key answers `502 placement_ambiguous` from then on: check the exchange before sending the order again with a new key.
- **Kept** — keys do not expire. A key is never reused for a different
  request, so mint a new one for each new order.
- Keys are scoped per-user — the same key from a different user does not
  collide.

The request body fingerprint is a SHA-256 of the recursively key-sorted
JSON of the body params. Reordering JSON keys between retries (`{a, b}` vs
`{b, a}`) does **not** trigger `idempotency_key_reused`.

---

## 6. CSV downloads — the non-JSON endpoints

`GET /api/v1/transactions/export` and
`GET /api/v1/tax/reports/:country/:year/download` do not return the JSON
envelope on success. They serve `text/csv` directly with an attachment
disposition. **Errors still use the JSON envelope**, so clients can branch on
`Content-Type`.

### Success response

```http
HTTP/1.1 200 OK
Content-Type: text/csv
Content-Disposition: attachment; filename="account_transactions_2026-05-27.csv"
X-Total-Transactions: 1234
X-Returned-Transactions: 1234
X-Truncated: false

date,entry_type,base_currency,base_amount,...
2026-05-26 14:32:00,buy,BTC,0.5,...
...
```

Custom headers (transactions export only):

- `X-Total-Transactions` — total matching rows (before cap)
- `X-Returned-Transactions` — rows actually included in the body (capped at 5000)
- `X-Truncated` — `"true"` if the result was capped; narrow with date filters

### Error responses (still JSON)

- `404 no_transactions` — no rows matched the filters
- `404 exchange_not_found` — unknown `exchange_id`
- `422 invalid_date` — bad `from_date`/`to_date` (use `YYYY-MM-DD`)
- `403 tool_disabled` — REST toggle off for `export_transactions_csv`, or not
  granted to this client

---

## 7. End-to-end smoke

```bash
HOST=https://your.deltabadger.com

# 1. Register a client requesting both scopes.
curl -X POST $HOST/oauth/register \
  -H 'Content-Type: application/json' \
  -d '{"client_name":"smoke","redirect_uris":["http://localhost:8765/callback"],"scope":"api mcp"}'

# 2. Run the authorize+token flow in a browser/CLI (PKCE). You now have $TOKEN.

# 3. Confirm the token works against REST.
curl -H "Authorization: Bearer $TOKEN" $HOST/api/v1/bots
curl -H "Authorization: Bearer $TOKEN" $HOST/api/v1/tax/jurisdictions

# 4. Place an order (after enabling `market_buy` in Settings → REST API).
curl -X POST $HOST/api/v1/orders \
  -H "Authorization: Bearer $TOKEN" \
  -H "Idempotency-Key: $(uuidgen)" \
  -H "Content-Type: application/json" \
  -d '{"type":"market_buy","exchange_name":"Binance","base_asset":"BTC","quote_asset":"USD","amount":100}'

# 5. The same token works against MCP because it carries both scopes.
#    Open a session; the response's Mcp-Session-Id header names it.
curl -i -X POST $HOST/mcp \
  -H "Authorization: Bearer $TOKEN" \
  -H 'Content-Type: application/json' \
  -H 'Accept: application/json, text/event-stream' \
  -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"smoke","version":"1"}}}'
SESSION='paste-the-Mcp-Session-Id'

curl -X POST $HOST/mcp \
  -H "Authorization: Bearer $TOKEN" -H "Mcp-Session-Id: $SESSION" \
  -H 'Content-Type: application/json' -H 'Accept: application/json, text/event-stream' \
  -d '{"jsonrpc":"2.0","method":"notifications/initialized"}'

curl -X POST $HOST/mcp \
  -H "Authorization: Bearer $TOKEN" -H "Mcp-Session-Id: $SESSION" \
  -H 'Content-Type: application/json' -H 'Accept: application/json, text/event-stream' \
  -d '{"jsonrpc":"2.0","id":2,"method":"tools/list"}'
```

---

## 8. MCP

The MCP server at `/mcp` speaks JSON-RPC over HTTP POST, protocol version
`2025-11-25`. It shares the tools of the endpoint table above.

- **Requests.** Every POST needs `Content-Type: application/json` (else
  `415`) and an `Accept` header naming both `application/json` and
  `text/event-stream` (else `406`). Start with `initialize`, without a session
  header; its answer carries `Mcp-Session-Id`. Send that header on every later
  request, and send the `notifications/initialized` notification before any
  other call. `DELETE /mcp` with the header ends the session.
- **Tokens.** A token is refused with HTTP `401` and JSON-RPC error `-32000`.
  This includes a token without the `mcp` scope (see Scopes).
- **Tool toggles.** A tool that is switched off for MCP, or not granted to the
  client, is left out of `tools/list`. Calling it answers JSON-RPC error
  `-32602` ("Tool '…' not found or not registered for this session"), not an
  envelope code. A switch takes effect on the next request.
- **Paper Trading.** With **Paper Trading** on (**Settings → MCP Server**),
  the order tools (`market_buy`, `market_sell`, `limit_buy`, `limit_sell`,
  `cancel_order`), `liquidate_exited_asset` and `answer_redeploy_offer` answer
  with a `[DRY RUN]` prefix and change nothing at the exchange.
  `answer_unrecorded_order` is refused with a `[DRY RUN]` message and records
  nothing, since the answer changes what the bot buys next.
  `create_bot`, `create_index_bot`, `create_signal_bot`, `start_bot` and
  `start_rule` are refused with a `[DRY RUN]` message, since the bot or rule
  would go on to move real money. Every other tool runs as usual. REST is
  never a dry run.
- **Arguments.** Each tool's input schema in `tools/list` is the reference,
  and it can be narrower than the REST fields. Lists are strings:
  `create_bot`'s `assets` and `update_bot_settings`' `allocations` take the
  `"BTC:60,ETH:40"` form only, and `liquidate_exited_asset` takes one `symbol`
  per call. `set_transaction_price` clears a stated price only with `""`.
- **Health check.** `GET /mcp/up` answers `{"status":"up","timestamp":"…"}`
  with `Accept: application/json`, and a plain page otherwise. It needs an
  `Authorization: Bearer …` header, but does not check the token.
