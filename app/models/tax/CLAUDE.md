# Tax Report System

## Architecture

Data-driven jurisdiction config (`Tax::Jurisdictions::REGISTRY`) + pluggable calculation engines + shared price service.

## Jurisdiction Config Flags

```ruby
'XX' => {
  name: 'Country',           # Display name
  method: :fifo,              # Calculation engine (see below)
  currency: 'EUR',            # Report currency
  locale: :en,                # CSV header language

  # Optional flags:
  crypto_to_crypto_taxable: false,  # Filter out swap disposals, chain cost basis
  stablecoin_ambiguous: true,       # Show "treat stablecoins as fiat" checkbox
  holding_exemption: 1.year,        # Tax-free after holding period
  swap_resets_holding_period: true,  # Swaps reset exemption clock but keep cost basis
  old_stock_cutoff: Date.new(2021, 3, 1),  # Pre-cutoff + >1yr = exempt
  short_long_term: true,            # Add short/long term classification
  tax_rate: { before: '26%', after: '33%', cutoff: Date.new(2026, 1, 1) },
  exemption_threshold: { max_year: 2024, amount: 2000 },  # Per-year gain threshold
  loss_deduction_rate: 0.7,         # Losses only X% deductible (Sweden)
  annual_exemption: 1270,           # Annual gain exemption (Ireland)
  split_payment: true,              # Split payment deadlines (Ireland)
  wealth_tax: { 2026 => { allowance: 59357, deemed_return: 0.0778, rate: 0.36 } },
  snapshot_date: :end_of_year,      # Dec 31 instead of Jan 1 (Switzerland)
  summary_only_total: true,         # No tax calc in summary (Switzerland)
  expense_deduction: 0.1,           # Automatic expense deduction on gains (Bulgaria)
  currency_by_year: { 2025 => 'BGN' },  # Override currency for specific years
  flat_tax_rate: 0.15,             # Flat tax rate with summary (Greece)
}
```

## Calculation Engines

| Engine | Class | Used By |
|--------|-------|---------|
| FIFO | `Tax::Methods::Fifo` | DE, AT, ES, BG, GR, PT, US |
| FIFO + 4-week | `Tax::Methods::Fifo4Week` | IE |
| LIFO | `Tax::Methods::Lifo` | IT |
| PVCT | `Tax::Methods::Pvct` | FR |
| Weighted Average | `Tax::Methods::WeightedAverage` | SE |
| Share Pooling | `Tax::Methods::SharePooling` | GB |
| Wealth Snapshot | `Tax::Methods::WealthSnapshot` | NL, CH |

## Adding a New Country

1. Add entry to `REGISTRY` in `app/models/tax/jurisdictions.rb`
2. If using existing engine with flags, done
3. If new engine needed: create `app/models/tax/methods/new_engine.rb`, register in `Jurisdictions.method_class`
4. Add localized headers to `config/locales/tax_report.XX.yml` (or existing base locale)
5. Add tests in `test/models/tax/methods/`

## Price Service Flow

1. `PriceService#prefetch` — scans transactions, loads from `historical_prices` DB table first
2. Missing prices fetched from CoinGecko in bulk (one API call per coin for full date range)
3. New prices saved to `historical_prices` for future reports
4. Failed lookups tracked in `@warnings`, appended to CSV

## Key Design Decisions

- Jurisdiction config is a hash, not classes — adding a country is one line
- Engines accept `**options` — flags passed through from config
- `crypto_to_crypto_taxable: false` makes FIFO chain cost basis through swaps via `group_id`
- Wealth snapshot engines skip per-transaction price enrichment entirely
- Historical prices persisted permanently (immutable reference data)
