# Tax Report System

Two reports, one ledger (`AccountTransaction`):

- **Crypto** — `Tax::Report`, every jurisdiction in `Tax::Jurisdictions::REGISTRY`, engine per method.
- **German broker** — `Tax::BrokerReport`, Anlage KAP / KAP-INV from an Alpaca account. DE only.

`Tax::GenerateReportJob` runs both (`SCOPES`, one filename builder, one concurrency key per scope).
The two are exact complements: `Tax::CryptoScope#crypto?` decides per **symbol**, not per venue —
Alpaca trades both — and the job and the broker report ask the same predicate.

## Architecture

Data-driven jurisdiction config (`Tax::Jurisdictions::REGISTRY`) + pluggable calculation engines +
shared price service.

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
| FIFO | `Tax::Methods::Fifo` | DE, AT, ES, BG, GR, PT, US, DK, CZ |
| FIFO + 4-week | `Tax::Methods::Fifo4Week` | IE |
| LIFO | `Tax::Methods::Lifo` | IT |
| PVCT | `Tax::Methods::Pvct` | FR |
| Weighted Average | `Tax::Methods::WeightedAverage` | SE |
| Share Pooling | `Tax::Methods::SharePooling` | GB |
| Wealth Snapshot | `Tax::Methods::WealthSnapshot` | NL, CH |

`Lifo` and `Fifo4Week` subclass `Fifo`. Every engine but `WealthSnapshot` includes `AcquisitionFee`
and `ReturnOfCapital`; `SharePooling` and `WeightedAverage` also include `PooledHoldings`.

## Adding a New Country

1. Add entry to `REGISTRY` in `app/models/tax/jurisdictions.rb`
2. If using existing engine with flags, done
3. If new engine needed: create `app/models/tax/methods/new_engine.rb`, register in `Jurisdictions.method_class`
4. Add localized headers under `tax_report.*` — split across `config/locales/tax_report.XX.yml` and
   `base.XX.yml`; the newer sections (`income`, `broker`) live in `base`
5. Add tests in `test/models/tax/methods/`

## Ledger Entry Types

`AccountTransaction.entry_type` 0–12 are the crypto basics (buy/sell/swap/deposit/withdrawal/
income/fee/lost). The broker-driven ones:

| Type | Meaning | Engines |
|---|---|---|
| `withholding_tax` (13) | Foreign tax withheld at source (Alpaca `DIVNRA`/`DIVFT`/`INTTW`…) | Inert in crypto engines; broker report sums it into KAP Z41 |
| `return_of_capital` (14) | `DIVROC` — reduces cost basis, never income | Basis floored at 0; the unabsorbed excess is a realised gain |
| `adjustment` (15) | **Splits only** (Alpaca `SPLIT`/`SSP`, remove+add legs merged into one signed net delta) | Rescales lot quantity and per-unit cost; acquisition date preserved |
| `unsupported_activity` (16) | Every activity type not modelled: mergers, spinoffs, option legs, and `JNLS`/`ACATS` share journals | **Inert in every engine** |

A share transfer in or out (`JNLS`, `ACATS`) is `unsupported_activity`, not `adjustment`: no
transferred-lot basis comes back from the API, and scaling the pool to fit would fabricate one.
Cash journals (`JNLC`, `OCT`, `ACATC`) are ordinary deposits/withdrawals.

`unsupported_activity` is inert on purpose — half-applying a merger corrupts the share count
silently. Instead the broker report **refuses** the symbol, and the crypto report leaves it out.

Crypto reports floor a return of capital at zero basis and never surface the excess — the engines
expose it as `excess_roc`, which nothing reads today. The broker report computes its own excess
through `LotLedger` and books it (KAP Z19 / KAP-INV distributions).

## Transfers Between the User's Own Accounts

`AccountTransaction#linked_transaction_id` points from a **withdrawal** to its **deposit**.
`TransferMatcher` auto-links conservatively — same asset, deposit within 72h after the withdrawal,
amount shrunk by 0–2% (the network fee) — and never overwrites a manual link. Unlinking in the
tracker sets `transfer_link_rejected`, or the next sync would re-link what the user just undid.
Validations refuse a link whose deposit is a different user, currency, direction, earlier in time,
or *larger* than the withdrawal.

Semantics every engine follows:

- A **withdrawal is not a disposal**. Linked, its lots stay and only the fee slice
  (`transfer_fee_amount`) leaves the pool at zero gain. Unlinked, the coins are assumed to sit in an
  unsynced wallet — the lots stay, and a later synced sale consumes them.
- A **linked deposit is a no-op** — the withdrawal never removed the lots.
- An **unlinked deposit opens a lot** priced at market on arrival with `basis_assumed: true`; those
  rows are named in the report's deposit-basis warning so the user can link them.
- Withdrawals are never priced (`PriceService#skip_pricing?`) — nothing reads the value, and a
  failed lookup would banner the whole report as incomplete.

## Price and FX Flow

1. `PriceService#prefetch` — scans transactions, loads from the `historical_prices` table first
2. Missing crypto prices fetched from CoinGecko/data-api in bulk (one call per coin, full range)
3. New prices saved to `historical_prices` for future reports (immutable reference data)
4. Fiat↔fiat uses **ECB euro reference rates**, not CoinGecko: `Tax::EcbFxRates`, SDMX csvdata
   endpoint (Bundesbank REST as fallback), whole history since 1999 stored in `fx_rates`
5. `EcbFxRates.rate(from:, to:, date:)` is a **multiplier**: `amount_to = amount_from * rate`.
   Weekends/holidays fall back to the previous business day; beyond `LOOKBACK_DAYS` (7) it raises
   `MissingRate`, and `PriceService` then approximates via a BTC cross and warns per affected record
6. Failed lookups collect in `@warnings` → incomplete banner + warning block in the CSV

`EcbFxRates.ensure_loaded!` runs from `PriceService#initialize`, **not** from `prefetch`. Load-bearing:
the wealth-snapshot branch never calls `prefetch`, and its stablecoin valuation needs a rate.

Two conversion paths coexist by design. The crypto report multiplies by `EcbFxRates.rate`, i.e. by a
reciprocal for USD→EUR; the broker report divides by the stored USD-per-EUR quote. The second is
exact, the first can differ in the last digit.

`stock_price_range` is the stock/ETF price path (namespaced `stock:SYM` in the same table, fetched
from the broker's own candles, converted to EUR). It keeps fetching until the window reaches its last
weekday and returns `{}` on failure — a boundary price is never guessed, the symbol is refused.

## Sync Watermark

`AccountTransactionSync` re-fetches a 25h overlap and dedups. The rules that matter:

- The watermark is **derived from the fetched data** (`max transacted_at`), never `Time.current` —
  a clock watermark silently drops whatever the fetch did not return.
- It is **clamped behind the earliest row that failed to save**, so one malformed entry cannot become
  a permanent hole — on an uncapped venue. On a capped one the clamp only reaches back as far as
  `ledger_window`; a row skipped before that is never re-fetched.
- The window start is floored at `Exchange#ledger_window` — **`nil` by default, i.e. no floor**.
  Declare it only where the endpoint caps the window it returns: Binance/MEXC/Bitget `80.days`
  (~90-day cap), Bybit and KuCoin `7.days` (`/v5/execution/list`, `/api/v1/fills`). Those caps are
  measured *from start_time*, so a watermark parked at a quiet account's last trade would query a
  window ending before today and the account would go blind.
  - **The default must stay `nil`.** Alpaca, Kraken, Coinbase and Hyperliquid paginate a cursor from
    `start_time` to the present, and **there is no recurring ledger sync** — `AccountTransactionSync`
    runs only from `TrackerController#index` and `Transaction#after_create`. A floor there would drop
    every month between two tracker visits, and the watermark would then advance past them.
  - Residual on a capped venue: anchoring at the present costs everything between the cap and an
    older watermark. On a Bybit or KuCoin account quiet for longer than 7 days, **any row in that
    gap — fills included, not just transfers** — falls outside every window, and Bybit's 30-day
    deposit/withdraw cap does not help because the window now starts 7 days ago. Anchoring at the
    present is still right: the alternative is an account that never sees anything new again.
    Closing the gap needs chunked `endTime` requests, not a floor.
- Dedup is by `tx_id` scoped to **(user, exchange)**; a blank id is read as `nil` and dedups on
  (api_key, type, currency, amount, timestamp) instead. `ApiKey#sync_issue` (never synced / last
  error) banners at the top of the crypto report — a report that silently omits an exchange is the
  worst outcome for a tax document.

## Fiat Rows

`Report#taxable_entries` drops every fiat-base row before the engines — a Kraken EUR-funded buy
arrives as a `sell` of EUR, which every engine priced at 1.0 against lots the user never had. It runs
*after* enrichment because `attribute_quote_row_fees` first moves the fiat row's fee onto the crypto leg.

Consequence: **income sections must read the unfiltered `enriched` array.** Dividends and interest
are fiat-base (Alpaca books them with `base_currency: 'USD'`), so filtering first would silently drop
every one of them. Income rows are also valued at build time, before the incomplete banner is decided.

## German Broker Report

`Tax::BrokerReport` (calculation) → `Tax::BrokerReportCsv` (Zeile numbers, labels, disclosures).
It deliberately does not reuse the crypto engines: those aggregate away the matched lots,
year-boundary holdings and per-lot Vorabpauschale the form needs, and drop the fiat rows the
dividends live on. BigDecimal end to end; the renderer rounds once.

- `Tax::LotLedger` — FIFO lots that keep what the form needs: matched-lot worksheet rows,
  split-restated unit counts (`units_in_terms_of`), per-unit accrued VAP, and an explicit
  `uncovered` tail instead of raising on a disposal with no basis.
- `Tax::Vorabpauschale.for_lot` — gross, pre-Teilfreistellung §18 VAP. The caller owns the §18(3)
  year shift (year N-1 is declared in year N) and must not call it for lots sold before year end.
- `FundClassification` — per user, per symbol: `share` / `fund` (+ Teilfreistellung category) /
  `other_security`. `resolve` returns the persisted record or an **unsaved proposal** (`etf` →
  `other_fund`, `stock` → `share`), built off the class so no stray `user.save` persists a guess.
  `Asset#instrument_type` (`stock`/`etf`, from data-api's stock feed) is the only input.
- `Tax::CryptoScope` — one predicate, three ways out (the user's own classification, then a stock/ETF
  asset row, then the category check), because `assets.symbol` is not unique: a ticker colliding with
  a coin would silently drop a security from a signed form.

**Refusal-first.** Refusing a symbol's summary always beats a plausible wrong figure. A refused
symbol contributes nothing to KAP/KAP-INV but still appears in the worksheet, marked. Triggers:
`uncovered_disposal`, `unsupported_activity`, `pre_2018_fund_lot`, `missing_price`, `missing_fx`,
`missing_basiszins`. Only an *unclassified* symbol blocks every summary (`summaries_available`).
Withholding survives refusal — dropping a Z41 credit costs the taxpayer money invisibly.
Banner on `result[:complete]` (no warnings), not on `summaries_available`: a cash leg has no symbol
to refuse, and an all-refused report still returns an all-zero KAP.

## Maintenance Burdens

- **`Tax::Vorabpauschale::BASISZINS` is a hardcoded table, not a feed.** Add one line each January
  when the BMF publishes the new Basiszins, or that year's report raises `KeyError` (caught, and
  reported to the user as a refused symbol — so a stale table looks like broken data, not a missing
  constant).
- **`Tax::BrokerReport::SUPPORTED_YEARS` is `(2023..2025)`** — the form years whose Zeile numbers were
  verified against the actual Anlage KAP / KAP-INV. Extending it means verifying the new year's form,
  not widening the range.

## Known Limitations (deliberate)

- Alpaca non-trade activities carry `status` `executed`/`correct`/`canceled`. Canceled are skipped at
  import, but id-based dedup does not apply a later `correct` restatement incrementally.
  **A full re-sync does NOT heal it.** Nulling `last_synced_at` re-fetches the history, and
  `AccountTransactionSync` then skips every row whose `tx_id` is already stored — `next if
  duplicate?`, never an update. Repairing a stored row means a data migration that restates it from
  `raw_data`, as `20260815150050_renormalize_alpaca_activity_rows.rb` does, or deleting the rows
  first.
- A sync run that **skipped invalid rows still clears `last_sync_error`**, so `ApiKey#sync_issue`
  reports nothing and the report's banner does not fire for them. Deliberate: the run did succeed,
  and a banner nothing can clear is worse than none. The **error log is the only signal** —
  `healthcheck-logs` picks up the `Account transaction sync skipped invalid entry` line.
- **A disposal spanning lots of different ages is emitted as one row**, and the holding-period
  exemption is decided from the oldest lot alone (`fifo.rb#record_disposal`). Selling 2 BTC where
  one lot is over a year old and one is not reports the whole gain as §23 exempt. Same shape drives
  US short/long, SK 19%/7%, IT `old_stock?` and the Danish wash-sale date. Pre-existing, not
  introduced with the broker report. `LotLedger#dispose` already returns the per-lot match shape a
  fix would need.
- `wealth_snapshot.rb` is the only engine that still removes an **unlinked withdrawal's** coins from
  the holding, so NL box-3 and CH wealth may understate. For a gains engine keeping the coins merely
  defers tax; in a wealth snapshot it would tax them every year.
- An **in-kind fee** (`fee` with a crypto base) consumes inventory at zero proceeds and zero gain, so
  the consumed lot's basis disappears — right on inventory, an understatement on basis. Applies to
  Alpaca `CFEE`, Kraken `creator_fee` and Binance margin interest alike.
- **Interest received net of foreign tax** is declared net in Z19 while Z41 still claims the credit —
  no `INTTW` gross-up signal exists. Understates in the taxpayer's favour, and is asymmetric with the
  `DIVFT` gross-up applied to dividends.
- **USD cash-balance FX gains** are out of scope (§20(2) Nr.7, BMF 14.05.2025 Rz 131) and disclosed
  verbatim in the report footer, along with the US-ETF default, the FX source and the fee treatment.
- **Vorabpauschale start value** is the prior year's last price, not "der erste im Kalenderjahr
  festgesetzte Rücknahmepreis" (BMF 21.05.2019 Tz 18.4). The chosen convention makes the year chain
  continuous (start N ≡ end N−1); the effect is a fraction of a percent of a ~1.75% base. Disclosed.
- **Z41 claims the full amount withheld.** Alpaca withholds 30% without a W-8BEN while only 15% is
  creditable under the DE-US treaty, the rest being an IRS reclaim. No input tells the report which
  applies, so it discloses rather than guesses.
- Alpaca's daily regulatory **fee aggregate** cannot be attributed to a disposal, so it is disclosed
  as a standalone total rather than deducted under §20(4).
- **MCP tax tools are crypto-only.** No `report_scope` property, and none without a product decision:
  a broker run before every security is classified emits an all-zero Anlage KAP.

## Key Design Decisions

- Jurisdiction config is a hash, not classes — adding a country is one line
- Engines accept `**options` — flags passed through from config
- `crypto_to_crypto_taxable: false` chains cost basis through a swap via `group_id` (AT, PT, PL, SK).
  Not France: `Tax::Methods::Pvct` is pool-based and never reads `group_id`. `enrich` must keep
  passing `group_id` through — without it the chaining silently does nothing.
- Wealth snapshot engines skip per-transaction price enrichment entirely (raw entry type, currency,
  amount and timestamp only — they never see `linked`)
- Historical prices and FX rates persisted permanently (immutable reference data)

## Trap

Verifying a locale key with `I18n.t(key, raise: false)` is **unsound** — it returns the String
`"Translation missing: …"`, so an absent key passes. Use `raise: true`. Same class of trap as
`I18n.exists?`, which `config.i18n.fallbacks = true` answers true for via English.
