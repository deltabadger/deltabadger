## Solid Queue

Config: `config/queue.yml`, recurring tasks: `config/recurring.yml`

Exchange-specific queues (one thread each for rate limiting): `binance`, `kraken`, `coinbase`, `binance_us`
General queues: `mailers`, `api_keys_validation`, `starting_bots`, `low_priority`, `default`

## Bot Job Lifecycle

1. `Bot::ActionJob` — main execution loop, runs periodically per bot settings
2. Calls `bot.execute_action` which:
   - Updates status to `:executing`
   - Creates/submits orders via concerns
   - Sets status to `:waiting`
3. Schedules next run via `bot.next_interval_checkpoint_at`
4. Retries with exponential backoff on failure

## Key Jobs

- `Bot::FetchAndUpdateOrderJob` — poll exchange for order status
- `Bot::UpdateMetricsJob` — update bot performance metrics
- `Exchange::SyncAllTickersAndAssetsJob` — sync market data (recurring)
- `Asset::FetchAllAssetsDataFromCoingeckoJob` — fetch asset metadata (recurring)
