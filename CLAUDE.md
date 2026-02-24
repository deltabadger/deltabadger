Deltabadger is an open-source Dollar Cost Averaging (DCA) bot for cryptocurrency trading.

## Stack

- Ruby 3.4.8 / Rails 8.1
- Node.js 18.19.1
- Hotwire (Turbo + Stimulus) for frontend
- SQLite with Solid Queue (background jobs), Solid Cache, Solid Cable (websockets)
- Tauri 2.x (Rust) for desktop app
- Docker deployment supported
- Sass (.sass)

## Development

- when adding a new feature, write tests first, and present it to review
- Use Rails style guidelines: `.claude/rails.md`

- Always check if our stack doesn't have built in solution already
- After every change in dependencies or deployment look check Docker and Tauri if they need updates
- Environment variables: see `.env.example`
- Run `bin/rails test` after every change

## Core Domain Models

**Bot (STI base class)** — DCA bot types:
- `Bots::DcaSingleAsset` — one trading pair
- `Bots::DcaDualAsset` — rebalances between two assets
- JSON `settings` column with `store_accessor` for flexible configuration
- Status flow: `created` → `scheduled` → `executing` → `waiting` → (repeat) / `stopped` / `deleted`

**Exchange (STI base class)** — exchange integrations:
- `Exchanges::Binance`, `Exchanges::BinanceUs`, `Exchanges::Kraken`, `Exchanges::Coinbase`
- Each implements: `market_buy`, `limit_buy`, `get_balances`, `get_tickers_info`, etc.
- API clients: `app/services/exchange_api/clients/`

**Asset** — cryptocurrencies and fiat currencies
- Identified by `external_id` (CoinGecko ID)
- Many-to-many with Exchange through `ExchangeAsset`

**Ticker** — trading pairs on exchanges (e.g., BTC/USD on Kraken)

**Transaction** — trade records created by bots
- Statuses: `pending`, `submitted`, `failed`, `skipped`, `cancelled`

**ApiKey** — encrypted exchange API credentials (Rails ActiveRecord Encryption)

## Common Gotchas

1. **Settings JSON column** — Bots use JSON `settings` with `store_accessor`. Changes trigger `settings_changed_at` timestamp.
2. **Turbo Frame targeting** — DOM IDs via `dom_id(model, :suffix)` for Turbo broadcasts.
3. **Asset synchronization** — assets never deleted, only marked `available: false`. Tickers and ExchangeAssets follow same pattern.
