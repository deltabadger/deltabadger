# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Deltabadger is an open-source Dollar Cost Averaging (DCA) bot for cryptocurrency trading. It's a Rails 8.1 application with Hotwire (Turbo + Stimulus) frontend, SQLite database, and optional Tauri desktop app wrapper. The app manages automated trading bots across multiple exchanges (Binance, Kraken, Coinbase).

**Tech Stack:**
- Ruby 3.4.8 / Rails 8.1
- Node.js 18.19.1
- Hotwire (Turbo + Stimulus) for frontend
- SQLite with Solid Queue (background jobs), Solid Cache, Solid Cable (websockets)
- Tauri 2.x (Rust) for desktop app
- Docker deployment supported

## Development Commands

### Setup
```bash
bin/setup              # Install dependencies and prepare database
bundle install         # Install Ruby gems
yarn install          # Install Node packages
```

### Running the App
```bash
bin/dev               # Start all services (Rails + JS bundler + CSS watcher)
rails s               # Rails server only (background jobs run in-process)
yarn build:watch      # JavaScript bundler with live reload
bin/rails dartsass:watch  # CSS watcher
```

### Testing
```bash
bundle exec rspec                    # Run all tests
bundle exec rspec spec/models        # Run model tests
bundle exec rspec spec/path/to/file_spec.rb  # Run single file
bundle exec guard -c                 # Auto-run tests on file changes
```

### Database
```bash
bundle exec rails db:prepare         # Create and migrate database
bundle exec rails db:migrate         # Run pending migrations
bundle exec rails db:seed           # Seed database
bundle exec rails db:reset          # Drop, create, migrate, seed
```

### Code Quality
```bash
bundle exec rubocop                  # Run linter
bundle exec rubocop -a              # Auto-fix issues
bundle exec bundle-audit check      # Security audit
bundle exec lol_dba db:find_indexes # Find missing indexes
```

### Docker
```bash
docker compose up -d                 # Start with compose
docker compose -f docker-compose.build.yml up -d --build  # Build from source
docker compose logs -f              # View logs
```

### Tauri Desktop App
```bash
yarn tauri dev                      # Run in dev mode
yarn tauri build                    # Build production app
./setup.sh                          # First-time setup (macOS/Linux)
./start.sh                          # Start app (macOS/Linux)
```

## Architecture

### Core Domain Models

**Bot (STI base class)** - Single Table Inheritance for different bot types:
- `Bots::DcaSingleAsset` - DCA bot for one trading pair
- `Bots::DcaDualAsset` - DCA bot that rebalances between two assets
- Bots use JSON `settings` column for flexible configuration (store_accessor)
- Status flow: `created` → `scheduled` → `executing` → `waiting` → (repeat) / `stopped` / `deleted`

**Exchange (STI base class)** - Exchange integrations:
- `Exchanges::Binance`, `Exchanges::BinanceUs`, `Exchanges::Kraken`, `Exchanges::Coinbase`
- Each exchange implements abstract interface methods (market_buy, limit_buy, get_balances, etc.)
- Uses `app/services/exchange_api/` for API client wrappers

**Asset** - Cryptocurrencies and fiat currencies (Bitcoin, USD, etc.)
- Identified by `external_id` (CoinGecko ID)
- Many-to-many with Exchange through `ExchangeAsset`

**Ticker** - Trading pairs on exchanges (e.g., BTC/USD on Kraken)
- Belongs to Exchange, base_asset, and quote_asset
- Stores min/max sizes, decimals, pricing info

**Transaction** - Trade records created by bots
- Tracks orders: amount, price, status, exchange response
- Statuses: `pending`, `submitted`, `failed`, `skipped`, `cancelled`

**ApiKey** - Encrypted exchange API credentials
- Encrypted with `attr_encrypted` gem using `APP_ENCRYPTION_KEY`
- Types: `:trading`, `:withdrawal`

### Concerns-Based Architecture

Following Jorge Manrubia's Rails guidelines (see `.cursor/rules/rails_style.mdc`):

**Bot Concerns** (`app/models/bot/`):
- Each concern is a decorator that extends bot functionality
- Concerns hook into lifecycle: `parse_params`, `execute_action`, `start`, `stop`
- Examples: `SmartIntervalable`, `LimitOrderable`, `PriceLimitable`, `Schedulable`
- Concerns are **composable traits**, not arbitrary code containers

**Exchange Concerns** (`app/models/exchange/`):
- `Synchronizer` - Syncs tickers/assets with CoinGecko data
- `CandleBuilder` - Builds candlestick data for charts
- `Dryable` - Dry-run mode for testing

**Shared Concerns** (`app/models/concerns/`):
- `ObfuscatesId` - ID obfuscation with Sqids
- `DomIdable` - DOM ID helpers for Turbo
- `Labelable` - Bot labeling logic
- `Undeletable` - Soft delete pattern

### Background Jobs

**Solid Queue** replaces Sidekiq (config: `config/queue.yml`):
- Exchange-specific queues (one thread each for rate limiting): `binance`, `kraken`, `coinbase`, `binance_us`
- General queues: `mailers`, `api_keys_validation`, `starting_bots`, `low_priority`, `default`
- Recurring tasks in `config/recurring.yml` (replaces sidekiq-cron)

**Bot Job Lifecycle:**
1. `Bot::ActionJob` - Main execution loop, runs periodically per bot settings
2. Calls `bot.execute_action` which:
   - Updates status to `:executing`
   - Creates/submits orders via concerns
   - Sets status to `:waiting`
3. Schedules next run via `bot.next_interval_checkpoint_at`
4. Retries with exponential backoff on failure

**Other Key Jobs:**
- `Bot::FetchAndUpdateOrderJob` - Poll exchange for order status
- `Bot::UpdateMetricsJob` - Update bot performance metrics
- `Exchange::SyncAllTickersAndAssetsJob` - Sync market data (recurring)
- `Asset::FetchAllAssetsDataFromCoingeckoJob` - Fetch asset metadata (recurring)

### Frontend Architecture

**Hotwire (NO React/Vue):**
- Turbo Frames for partial page updates
- Turbo Streams for real-time updates (via Action Cable)
- Stimulus controllers in `app/javascript/controllers/`
- All JavaScript bundled with esbuild

**Key Stimulus Controllers:**
- `app_wake_controller.js` - Keeps app connection alive
- `bot/*_controller.js` - Bot-specific interactions
- `form/*_controller.js` - Form handling
- `hotwire_animations_controller.js` - Turbo Frame animations

**Styling:**
- ALL CSS in `app/assets/stylesheets/`
- MUST use `.sass` format (indented syntax), never `.scss` or `.css`
- Asset pipeline managed by Sprockets + Dartsass

### Services Pattern

Despite `.cursor/rules/rails_style.mdc` discouraging service objects, this codebase DOES use them in `app/services/`:
- `BaseService` - Base class with `.call` pattern
- `Result` - Success/Failure monad for error handling
- Exchange API clients in `app/services/exchange_api/clients/`
- Market data fetchers in `app/services/exchange_api/markets/`

**Result Monad Pattern:**
```ruby
result = SomeService.call(params)
return result if result.failure?
# Use result.data
```

### Tauri Desktop Integration

- Rails runs as background process managed by Tauri
- `app/javascript/tauri.js` - IPC bridge between Rails and Tauri
- Rust code in `src-tauri/` handles OS integration, tray icon, window management
- Desktop app shares same Rails backend as web version

### Authentication & Security

- Devise for authentication
- Optional 2FA with `active_model_otp`
- Encrypted API keys using `attr_encrypted` with `APP_ENCRYPTION_KEY`
- Multi-tenant: each user has their own bots and API keys

### Internationalization

- Supported locales: `[:en, :pl, :es, :de, :nl, :fr, :pt, :ru, :it]`
- Translations in `config/locales/`
- Locale selector in user settings
- Routes scoped by locale: `/:locale/bots`

### Database

- SQLite3 with proper connection pooling
- Three separate SQLite databases:
  - Main DB: `storage/production.sqlite3`
  - Solid Queue: `storage/production_queue.sqlite3`
  - Solid Cache: `storage/production_cache.sqlite3`
- Schema managed in `db/schema.rb`
- Migrations in `db/migrate/`

## Key Files & Locations

### Configuration
- `config/queue.yml` - Solid Queue worker/dispatcher config
- `config/recurring.yml` - Recurring background jobs
- `config/puma.rb` - Web server config
- `docker-entrypoint.sh` - Docker startup logic, auto-generates secrets

### Bot Creation Flow
Controllers handle multi-step wizard:
1. `Bots::DcaSingleAssets::PickBuyableAssetsController`
2. `Bots::DcaSingleAssets::PickExchangesController`
3. `Bots::DcaSingleAssets::AddApiKeysController`
4. `Bots::DcaSingleAssets::PickSpendableAssetsController`
5. `Bots::DcaSingleAssets::ConfirmSettingsController`

### Exchange Implementations
- `app/models/exchanges/binance.rb` - Binance API logic
- `app/models/exchanges/kraken.rb` - Kraken API logic
- `app/models/exchanges/coinbase.rb` - Coinbase API logic
- Each implements: `market_buy`, `get_balances`, `get_tickers_info`, etc.

### API Clients
- `app/services/exchange_api/clients/` - Faraday HTTP clients for each exchange
- `app/services/exchange_api/markets/` - Market data fetchers
- `app/services/exchange_api/validators/` - API key validators

## Important Patterns & Conventions

### Rails Style Guidelines (from `.cursor/rules/rails_style.mdc`)

**File Structure:**
- Use vanilla Rails structure, no arbitrary folders
- Model concerns: `app/models/concerns/` (shared) or `app/models/<model>/` (model-specific)
- Controller concerns: `app/controllers/concerns/` or `app/controllers/concerns/<subsystem>/`
- Concerns must be cohesive units with "has trait" semantics, not arbitrary code containers

**Dependencies:**
- Resist adding Ruby gems - keep Gemfile minimal
- Resist adding JS packages even more - NO React/Vue/Angular
- Prefer Rails built-in solutions

**Frontend:**
- Use Hotwire (Turbo + Stimulus) exclusively
- Embrace server-side rendering with ERB
- Turbo Streams for real-time updates

### Styling Rules (from `.cursor/rules/styles.mdc`)

**CRITICAL:**
- ALL CSS in `app/assets/stylesheets/`
- ALWAYS use `.sass` format (indented syntax)
- NEVER use `.scss` or `.css`
- New feature styles go in `app/assets/stylesheets/new/`

### DRY_RUN Mode

- Set `DRY_RUN=true` in `.env` for development
- When enabled, exchanges return mocked data instead of real API calls
- Production always forces `DRY_RUN=false`
- Test always forces `DRY_RUN=true`
- Implemented in `app/models/exchange/dryable.rb`

### Order Flow

1. Bot scheduled via `Bot::ActionJob`
2. Job calls `bot.execute_action`
3. Concern decorators modify behavior (limits, smart intervals, etc.)
4. `OrderCreator#set_order` creates Transaction
5. Exchange API called to submit order
6. `Bot::FetchAndUpdateOrderJob` polls for completion
7. Turbo Streams broadcast updates to UI

### Real-Time Updates

- Action Cable for websockets (`mount ActionCable.server => '/cable'`)
- Turbo Streams broadcast to user-specific channels: `["user_#{user_id}", :bot_updates]`
- See `app/models/bot.rb` broadcast methods: `broadcast_status_bar_update`, `broadcast_new_order`

### CoinGecko Integration

**CoinGecko is now optional** - the app uses pre-seeded fixture data by default:
- Fixtures in `db/fixtures/` contain top 100 cryptocurrencies + fiat currencies + tickers
- Loaded automatically during `rails db:seed` via `SeedDataLoader`
- No API key required for basic functionality

**Optional CoinGecko API key** (for fresh market data):
- Add `COINGECKO_API_KEY` to enable live sync of assets and tickers
- When configured: recurring jobs sync every 12 hours
- When not configured: jobs skip silently, app uses fixture data
- `Coingecko` model wraps API client (`app/models/coingecko.rb`)
- Caching: 60s for prices, 6h for market caps

**Generating new fixtures** (requires CoinGecko API key):
```bash
# Generate all fixtures (assets + tickers for all exchanges)
rake fixtures:generate_all

# Generate only assets (top 100 cryptocurrencies + fiat)
rake fixtures:generate_assets

# Generate tickers for specific exchange
rake fixtures:generate_tickers[binance]
```

**Fixture data includes:**
- `assets.json` - Top N cryptocurrencies (default: 100) + 17 fiat currencies
  - Includes `circulating_supply` for dynamic market cap calculation
  - Market cap calculated as: circulating_supply × current_price (from exchange)
  - Circulating supply changes slowly, so fixtures remain accurate
- `tickers/{exchange}.json` - Trading pairs with min/max sizes, decimals, pricing info
- Change `Fixtures::AssetsGenerator::TOP_N_CRYPTOCURRENCIES` to include more/fewer assets

**Market cap allocation (DCA Dual Asset bots):**
- Without CoinGecko: Uses `circulating_supply` × current exchange price
- With CoinGecko: Can fetch live market cap directly
- Circulating supply from fixtures + live prices = accurate dynamic market cap

## Environment Variables

Key variables (see `.env.example`):
- `SECRET_KEY_BASE`, `DEVISE_SECRET_KEY` - Rails secrets
- `APP_ENCRYPTION_KEY` - 32-char hex for encrypting API keys
- `DRY_RUN` - Mock exchange APIs (development only)
- `ORDERS_FREQUENCY_LIMIT` - Minimum seconds between orders
- `COINGECKO_API_KEY` - Optional CoinGecko API key
- `APP_ROOT_URL`, `HOME_PAGE_URL` - App URLs
- SMTP settings for email notifications

Docker auto-generates secrets in `/app/storage/.secrets` on first run.

## Testing

- RSpec for testing
- FactoryBot for fixtures (`spec/factories.rb`)
- Capybara + Selenium for feature tests
- Test DB auto-maintained via `db:test:prepare`

## Common Gotchas

1. **Services exist despite style guide** - The codebase has `app/services/` even though `.cursor/rules/rails_style.mdc` discourages it. This is existing code. For new features, prefer models/concerns.

2. **Settings JSON column** - Bots use JSON `settings` with `store_accessor` for dynamic fields. Changes to settings trigger `settings_changed_at` timestamp.

3. **Concerns as decorators** - Bot concerns decorate methods like `parse_params` and `execute_action`. Order matters - concerns are included in dependency order in bot classes.

4. **Exchange queue isolation** - Each exchange has its own Solid Queue with 1 thread to prevent rate limit violations.

5. **Turbo Frame targeting** - DOM IDs generated via `dom_id(model, :suffix)` helper for Turbo broadcasts.

6. **Asset synchronization** - Assets never deleted, only marked `available: false`. Tickers and ExchangeAssets follow same pattern.

7. **macOS fork() crash** - Set `export OBJC_DISABLE_INITIALIZE_FORK_SAFETY=YES` if you see fork() errors during development.

## Deployment

**Docker (recommended):**
- Single command: `docker run -d --name deltabadger -p 3737:3000 -v deltabadger_data:/app/storage ghcr.io/deltabadger/deltabadger:latest standalone`
- Or use `docker-compose.yml` with `.env.docker` overrides

**Tauri (macOS/Linux):**
- Download release, run `./setup.sh`, then `./start.sh`
- App runs in system tray on macOS

## Mission Control

Job monitoring available at `/jobs` (admin only) - uses `mission_control-jobs` gem to manage Solid Queue.

## Commits

- Do not include Co-Authored-By lines in commit messages.
