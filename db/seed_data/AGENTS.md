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

## Catalogue bundle

`catalogue.json.gz` is data-api's public catalogue snapshot (stock assets, Alpaca stock and crypto listings, IBKR listings), not committed (`.gitignore`): the release workflow's `release-catalogue` job downloads it from `https://data.deltabadger.com/public/v1/catalogue.json.gz`, attaches it to the GitHub release and hands it to the image build; the desktop build downloads the same file. Without data-api, the Rust app imports it once per start (`catalogue_bootstrap`) and refreshes from the public snapshot daily (`catalogue_refresh`, `IBKR_CATALOGUE_URL=off` turns that off), each source through the revision-fenced importer, so an older bundle never overwrites a newer import. For a local build, download it into this directory with curl.

## Live Sync

Two providers, configured in settings:
- **CoinGecko** (open source users): `COINGECKO_API_KEY` enables direct sync
- **data-api** (deltabadger.com users): `MARKET_DATA_URL` + `MARKET_DATA_TOKEN`
- Recurring jobs sync every 12 hours when configured; skip silently when not

## Market-cap weighting

`Bots::DcaMultiAsset#apply_market_cap_weights` reads the stored `assets.market_cap` column (synced with the catalogue) — no live price is involved. `circulating_supply` in the seed data is informational.
