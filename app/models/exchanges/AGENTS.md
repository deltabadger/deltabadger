# Exchange Models

## API Key Validation (`get_api_key_validity`)

Trading keys and withdrawal keys are validated differently on purpose. A valid API key is not enough — we must confirm it has the correct permissions for its intended use.

- **Trading keys** must be validated with a trade-permission endpoint (e.g. `cancel_order` with a fake order ID, or checking permission flags like `canTrade`). A read-only endpoint would accept any valid key, even one without trade permissions.
- **Withdrawal keys** are validated with a read-only endpoint (e.g. `get_balances`, `get_accounts`). We only need to confirm the key is valid — withdrawal permission checks happen at withdrawal time.
- **Reading keys** (`key_type: :read_only`, what the tracker asks for) are routed by `ApiKey#get_validity` to `Exchange#get_read_api_key_validity`, shared by every venue: it reads balances, and a key that reads is valid. Override it only where the venue can report more, or the tracker needs more than a balance read — Kraken, whose ledger needs its own permission.
- **Venues that report a key's permissions** (Kraken's `GetApiKeyInfo`) compare them with the steps instead of probing: a table per key type of required and forbidden flags (`Exchanges::Kraken::KEY_PERMISSIONS`), answering `Result::Success.new(missing_permissions: [...], forbidden_permissions: [...])` when they do not match. `ApiKey` takes that as `:incorrect` and every key form names the permissions (`BotHelper#api_key_permission_message`), in the words of the steps: each flag's label lives beside the steps in `api.<locale>.yml` (`api_key_permissions.<venue>.<flag>`), and `test/helpers/api_key_permission_message_test.rb` checks every label appears in the steps rendered for its key type in every locale.

**Setup steps and checks must match.** Every permission a venue's steps ask for (or forbid) is one its check proves, and every permission its check requires is in its steps. Withdraw permission is the one exception where a venue cannot report it without a withdrawal.

Every exchange branches on `api_key.withdrawal?` in `get_api_key_validity` to select the appropriate validation strategy.

`key_type` is a **capability, not a category**: `trading` contains `read_only` (a key that may trade may already read), which is why a bot's key satisfies the tracker as it stands and nothing is ever converted. `withdrawal` is a scope of its own rather than a bigger `trading` — every venue's check requires trading to be OFF on one. `ApiKey.reading` is the subset query that follows, returning at most one key per venue.

## Humanizing exchange errors

Raw exchange error strings reach users via `Exchange#humanize_error`, called from `Bot::ActionJob#humanized_errors` before `notify_about_error`. The classifier lives in honeymaker (per-exchange `ERROR_PATTERNS`), the translations live in this app.

To add a friendly message for a new error:

1. In honeymaker, add a `{ code:, pattern: }` entry to the exchange's `ERROR_PATTERNS` (e.g. `lib/honeymaker/exchanges/kraken.rb`). Use named captures for values to interpolate.
2. Release honeymaker (`rake release`) and bump the version in this repo's `Gemfile`.
3. Add `errors.exchange.<code>` to every `config/locales/errors.*.yml` with native phrasing (no `default:`, per `config/locales/AGENTS.md`). The `humanize_error` call automatically passes the matched named captures plus `exchange: <name>` as interpolation params.

Unmatched errors fall through to the raw message — they still surface, just untranslated.
