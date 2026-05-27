# Deltabadger REST API

The REST API mirrors the MCP server's read / control / trade coverage. Both
surfaces share auth (Doorkeeper OAuth 2.1) and business logic, so changes to
one are reflected by the other.

- **Base URL:** `https://{your-subdomain}.deltabadger.com/api/v1`
- **Format:** `application/json` (with one documented exception: CSV export)
- **Auth:** OAuth 2.1 bearer token with `api` scope
- **State changes:** `POST /api/v1/orders` requires an `Idempotency-Key` header

The REST API is intentionally OAuth-only. Web-session cookies are not honored
on `/api/v1/*` — a signed-in browser session without a bearer header is
rejected as if unauthenticated.

---

## 1. Auth

There are two paths to a valid bearer token. Most users only need the first.

### 1a. First-party / personal scripts (the simple path)

Open **Settings → Connect → REST API**. Your personal API token is already
there — you don't have to create it. Copy the value.

Use it in your script:

```bash
TOKEN='paste-from-settings'
curl -H "Authorization: Bearer $TOKEN" \
     https://your.deltabadger.com/api/v1/bots
```

If you ever suspect the token has leaked, click **Regenerate** in the same
widget. The current token is revoked immediately and a new one is shown.
Update your script with the new value.

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
is rejected at `/api/v1/*` (403 `insufficient_scope`); a token with only
`api` is rejected at `/mcp` (same code).

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
  "client_id": "abc123...",
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

### Discovery

- `GET /.well-known/oauth-authorization-server` (RFC 8414) — advertises
  `scopes_supported: ["mcp", "api"]`, endpoints, supported grant types,
  code challenge method, etc.
- `GET /.well-known/oauth-protected-resource` (RFC 9728) — points to the
  protected resource and the authorization servers.

---

## 2. Tool toggles

Every REST endpoint is gated by a per-user, per-tool toggle. **All toggles
default to off.** A request with a valid `:api` token whose user has not
enabled the matching toggle returns:

```http
HTTP/1.1 403 Forbidden
Content-Type: application/json

{
  "data": null,
  "error": { "code": "tool_disabled", "message": "Tool 'list_bots' is disabled for this user." }
}
```

Enable toggles from **Settings → Connect → REST API**. Toggles are grouped
(`read`, `control`, `trade`); each row is independent. REST toggles are
**isolated from MCP** — enabling `list_bots` for REST does not affect MCP
permissions and vice versa.

Tool names mirror the MCP names exactly: `list_bots`, `get_bot_details`,
`list_exchanges`, `get_exchange_balances`, `get_portfolio_summary`,
`list_transactions`, `list_open_orders`, `export_transactions_csv`,
`list_account_transactions`, `create_bot`, `start_bot`, `stop_bot`,
`update_bot_settings`, `start_rule`, `stop_rule`, `update_rule_settings`,
`market_buy`, `market_sell`, `limit_buy`, `limit_sell`, `cancel_order`.

Tax-report generation tools are MCP-only and intentionally out of REST scope.

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

**Exception:** `GET /api/v1/transactions/export` returns `text/csv` directly
on success. See section 6.

### HTTP status mapping

Errors set both an HTTP status and an envelope `error.code`. Common pairs:

| Status | Common error codes |
|---|---|
| 400 | `idempotency_key_required` |
| 401 | `missing_token`, `invalid_token`, `token_revoked`, `token_expired`, `user_not_found` |
| 403 | `tool_disabled`, `insufficient_scope`, `api_key_missing` |
| 404 | `bot_not_found`, `rule_not_found`, `exchange_not_found`, `pair_not_found`, `no_transactions` |
| 409 | `bot_already_running`, `bot_not_running`, `bot_running`, `rule_already_active`, `rule_not_active`, `rule_active`, `idempotency_in_progress`, `idempotency_key_reused` |
| 422 | `missing_required_parameter`, `invalid_interval`, `invalid_allocation`, `invalid_date`, `invalid_order_type`, `no_updates_provided`, `exchange_name_required`, `bot_invalid`, `bot_save_failed`, `rule_save_failed` |
| 502 | `order_failed`, `cancel_failed`, `balances_fetch_failed`, `bot_stop_failed` |

---

## 4. Endpoint table

All paths are under `/api/v1`. The "Tool" column names the REST toggle that
must be enabled for the call to succeed (in addition to a valid `:api`
token).

| Method | Path | Tool | Notes |
|---|---|---|---|
| GET | `/bots` | `list_bots` | Optional `?status=` filter |
| GET | `/bots/:id` | `get_bot_details` | Includes metrics if available |
| POST | `/bots` | `create_bot` | 201 on success; required: `exchange_name`, `base_asset`, `quote_asset`, `quote_amount`, `interval` |
| PATCH | `/bots/:id` | `update_bot_settings` | Accepts `quote_amount`, `label`; rule must be stopped |
| POST | `/bots/:id/start` | `start_bot` | 409 if already running |
| POST | `/bots/:id/stop` | `stop_bot` | 409 if not running |
| GET | `/exchanges` | `list_exchanges` | Lists user trading exchanges |
| GET | `/exchanges/:id/balances` | `get_exchange_balances` | Live exchange call; 502 on upstream failure |
| GET | `/transactions` | `list_transactions` | Optional `?bot_id=`, `?limit=` (max 100) |
| GET | `/transactions/account` | `list_account_transactions` | Optional `?exchange_id=`, `?from_date=`, `?to_date=`, `?entry_type=`, `?limit=` (max 200) |
| GET | `/transactions/export` | `export_transactions_csv` | **CSV** (see section 6) |
| GET | `/portfolio` | `get_portfolio_summary` | Returns `empty: true` for users with no bots |
| GET | `/orders` | `list_open_orders` | Optional `?exchange_name=`; merges DB + live exchange orders |
| POST | `/orders` | per-type | **Requires `Idempotency-Key`** (see section 5) |
| DELETE | `/orders/:id` | `cancel_order` | Numeric ID → DB row; non-numeric → exchange order (then `exchange_name` required) |
| POST | `/rules/:id/start` | `start_rule` | 409 if already active |
| POST | `/rules/:id/stop` | `stop_rule` | 409 if not active |
| PATCH | `/rules/:id` | `update_rule_settings` | Accepts `withdrawal_percentage`, `max_fee_percentage`, `min_amount`, `threshold_type`; rule must be stopped |

---

## 5. POST /api/v1/orders — idempotency

Order placement is the only state-changing endpoint that requires
idempotency. Cancellation (`DELETE /api/v1/orders/:id`) is intentionally
**not** idempotency-wrapped — cancelling an already-cancelled order is a
benign no-op at the exchange level.

### Request

`POST /api/v1/orders` dispatches on a required `type` param:

- `market_buy` — spend `amount` in `quote_asset` (default) or buy `amount`
  of `base_asset` if `amount_type=base`.
- `market_sell` — sell `amount` of `base_asset` (default) or receive
  `amount` in `quote_asset` if `amount_type=quote`.
- `limit_buy` / `limit_sell` — same shape plus a required `price`.

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
  an exchange-rejected order is a definitive outcome.
- **Conflicts:**
  - Same key, request **in progress** (concurrent retry) → `409 idempotency_in_progress`. We do not block-and-wait; retry shortly.
  - Same key, request **completed**, **different body** → `409 idempotency_key_reused`. To retry with a different payload, mint a new key.
- **TTL** — keys are retained for 24 hours, then swept. Long enough to
  cover any sane retry window, short enough to bound the table.
- Keys are scoped per-user — the same key from a different user does not
  collide.

The request body fingerprint is a SHA-256 of the recursively key-sorted
JSON of the body params. Reordering JSON keys between retries (`{a, b}` vs
`{b, a}`) does **not** trigger `idempotency_key_reused`.

---

## 6. CSV export — the one non-JSON endpoint

`GET /api/v1/transactions/export` is the only endpoint that does not return
the JSON envelope on success. It serves `text/csv` directly with an
attachment disposition. **Errors still use the JSON envelope**, so clients
can branch on `Content-Type`.

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

Custom headers:

- `X-Total-Transactions` — total matching rows (before cap)
- `X-Returned-Transactions` — rows actually included in the body (capped at 5000)
- `X-Truncated` — `"true"` if the result was capped; narrow with date filters

### Error responses (still JSON)

- `404 no_transactions` — no rows matched the filters
- `404 exchange_not_found` — unknown `exchange_id`
- `422 invalid_date` — bad `from_date`/`to_date` (use `YYYY-MM-DD`)
- `403 tool_disabled` — REST toggle off for `export_transactions_csv`

---

## 7. End-to-end smoke

```bash
# 1. Register a client requesting both scopes.
curl -X POST https://your.deltabadger.com/oauth/register \
  -H 'Content-Type: application/json' \
  -d '{"client_name":"smoke","redirect_uris":["http://localhost:8765/callback"],"scope":"api mcp"}'

# 2. Run the authorize+token flow in a browser/CLI (PKCE). You now have $TOKEN.

# 3. Confirm the token works against REST.
curl -H "Authorization: Bearer $TOKEN" https://your.deltabadger.com/api/v1/bots

# 4. Place an order (after enabling `market_buy` in Settings → Connect → REST API).
curl -X POST https://your.deltabadger.com/api/v1/orders \
  -H "Authorization: Bearer $TOKEN" \
  -H "Idempotency-Key: $(uuidgen)" \
  -H "Content-Type: application/json" \
  -d '{"type":"market_buy","exchange_name":"Binance","base_asset":"BTC","quote_asset":"USD","amount":100}'

# 5. The same token still works against MCP because it carries both scopes.
curl -H "Authorization: Bearer $TOKEN" -X POST https://your.deltabadger.com/mcp -d '...'
```
