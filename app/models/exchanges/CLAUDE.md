# Exchange Models

## API Key Validation (`get_api_key_validity`)

Trading keys and withdrawal keys are validated differently on purpose. A valid API key is not enough — we must confirm it has the correct permissions for its intended use.

- **Trading keys** must be validated with a trade-permission endpoint (e.g. `cancel_order` with a fake order ID, or checking permission flags like `canTrade`). A read-only endpoint would accept any valid key, even one without trade permissions.
- **Withdrawal keys** are validated with a read-only endpoint (e.g. `get_balances`, `get_accounts`). We only need to confirm the key is valid — withdrawal permission checks happen at withdrawal time.

Every exchange branches on `api_key.withdrawal?` in `get_api_key_validity` to select the appropriate validation strategy.

## Humanizing exchange errors

Raw exchange error strings reach users via `Exchange#humanize_error`, called from `Bot::ActionJob#humanized_errors` before `notify_about_error`. The classifier lives in honeymaker (per-exchange `ERROR_PATTERNS`), the translations live in this app.

To add a friendly message for a new error:

1. In honeymaker, add a `{ code:, pattern: }` entry to the exchange's `ERROR_PATTERNS` (e.g. `lib/honeymaker/exchanges/kraken.rb`). Use named captures for values to interpolate.
2. Release honeymaker (`rake release`) and bump the version in this repo's `Gemfile`.
3. Add `errors.exchange.<code>` to every `config/locales/errors.*.yml` with native phrasing (no `default:`, per `config/locales/CLAUDE.md`). The `humanize_error` call automatically passes the matched named captures plus `exchange: <name>` as interpolation params.

Unmatched errors fall through to the raw message — they still surface, just untranslated.
