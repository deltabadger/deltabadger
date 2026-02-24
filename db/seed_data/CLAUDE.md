## Seed Data

Pre-built JSON for first boot (no API key required):
- `assets.json` — cryptocurrencies + fiat currencies with metadata, colors, circulating supply
- `indices.json` — CoinGecko category indices with per-exchange top coins
- `tickers/{exchange}.json` — trading pairs (one file per exchange)

Loaded during `rails db:seed` via `MarketData.import_*` methods.

## One Code Path

`MarketData.import_assets!`, `import_indices!`, `import_tickers!`:
- Used by `db/seeds.rb` (reads JSON files at first boot)
- Used by `sync_*_from_deltabadger!` (fetches from data-api HTTP during live sync)
- Same upsert logic, different data sources

## Regenerating Seed Data

Requires CoinGecko API key:
```bash
COINGECKO_API_KEY=xxx rake seed:generate
```
Syncs from CoinGecko + exchange APIs into a temp DB, then exports as JSON.

## Live Sync

Two providers, configured in settings:
- **CoinGecko** (open source users): `COINGECKO_API_KEY` enables direct sync
- **data-api** (deltabadger.com users): `MARKET_DATA_URL` + `MARKET_DATA_TOKEN`
- Recurring jobs sync every 12 hours when configured; skip silently when not

## Market Cap Allocation (DCA Dual Asset)

Uses `circulating_supply` from seed data x current exchange price.
Circulating supply changes slowly, so seed data remains accurate.
