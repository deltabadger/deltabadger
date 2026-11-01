# The Rails half of the tax report's parity (rust/tests/tax_parity.rs is the Rust half).
#   bin/rails runner script/rust/tax_vectors.rb rust/tests/fixtures/tax_vectors.json
# Run from a clean export (git archive), never a working install: each scenario is built on its own scratch SQLite file
# with the app's own models, Tax::GenerateReportJob#perform runs exactly as the queue would run it, and the file it
# writes (or the refusal beside it) is recorded with every table row the Rust half reads. Re-run when a file under
# "ported_sources" changes.
#
# Each scenario says what Rust must do with it: "parity" (the same bytes, or the same refusal), or "diverges: D-TAX-…"
# where Rails states a figure that cannot be defended or leaves out what the user must be told: Rust's own CSV is then
# rust/tests/fixtures/tax_divergences/<name>.csv, checked by hand, and the Rails file is recorded beside it for reference.
# Scenarios run in the report's country (default US), year (default 2026) and stablecoin flag (default false); the ECB
# table holds a rate for every day a row was booked on, less the scenario's `fx_gap` days.
require 'json'
require 'digest'
require 'fileutils'
require 'tmpdir'
require 'active_support/testing/time_helpers'
require_relative 'schema_template'

ENV['TZ'] = 'UTC'

module Harness
  class Unscripted < Exception; end # rubocop:disable Lint/InheritException
end
Net::HTTP.prepend(Module.new { def connect = raise(Harness::Unscripted, "real connection to #{address}:#{port}") })

# No broadcast reaches anyone, and the job's half-second wait for one is skipped.
Turbo::StreamsChannel.singleton_class.prepend(Module.new { def broadcast_replace_to(*) = nil })
Tax::GenerateReportJob.prepend(Module.new { def sleep(*) = nil })
# A price the table lacks is a price nobody has: the provider answers with a failure, as an unreachable one does.
MarketData.singleton_class.prepend(Module.new do
  def get_historical_price_range(**) = Result::Failure.new('unavailable')
end)

module TaxVectors
  extend ActiveSupport::Testing::TimeHelpers
  module_function

  NOW = Time.utc(2026, 10, 1, 3, 0, 0)
  ENTRY = AccountTransaction.entry_types
  TABLES = %w[users exchanges assets tickers api_keys account_transactions historical_prices fund_classifications fx_rates].freeze
  PORTED = %w[
    app/jobs/tax/generate_report_job.rb app/models/tax/report.rb app/models/tax/methods/fifo.rb
    app/models/tax/methods/acquisition_fee.rb app/models/tax/price_service.rb app/models/tax/crypto_scope.rb
    app/models/tax/asset_identity.rb app/models/tax/jurisdictions.rb app/models/api_key.rb app/models/concerns/csv_safe.rb
    app/models/tax/methods/return_of_capital.rb app/models/tax/ecb_fx_rates.rb app/services/bot_api/tax/list_jurisdictions.rb
    app/models/tax/methods/lifo.rb app/models/tax/methods/fifo4_week.rb
  ].freeze

  # The schema is loaded before travelling: a migration stamped after NOW would not load.
  def schema(file) = SchemaTemplate.load_primary(file)

  def build
    user = User.new(name: 'Owner', email: 'owner@example.com', password: 'correct horse battery staple', admin: true,
                    confirmed_at: Time.current, setup_completed: true)
    user.save!(validate: false)
    alpaca = Exchanges::Alpaca.create!(name: 'Alpaca', maker_fee: '0.15', taker_fee: '0.25')
    assets = {}
    asset = ->(symbol, ext, category, type = nil) { assets[symbol] = Asset.create!(external_id: ext, symbol:, name: symbol, category:, instrument_type: type) }
    asset.('USD', 'usd', 'Fiat')
    asset.('AAPL', 'AAPL.US', 'Stock', 'stock')
    asset.('BTC', 'bitcoin', 'Cryptocurrency')
    asset.('ETH', 'ethereum', 'Cryptocurrency')
    asset.('SOL', 'solana', 'Cryptocurrency')
    assets.each_value { |a| ExchangeAsset.create!(exchange: alpaca, asset: a, available: true) }
    asset.('USDT', 'tether', 'Cryptocurrency')
    asset.('BNB', 'binancecoin', 'Cryptocurrency')
    sizes = { base_decimals: 9, quote_decimals: 2, price_decimals: 2, minimum_base_size: '0.000000001', minimum_quote_size: '1' }
    { 'AAPL' => 'AAPL', 'BTC' => 'BTC/USD', 'ETH' => 'ETH/USD', 'SOL' => 'SOL/USD' }.each do |s, t|
      Ticker.create!(exchange: alpaca, ticker: t, base: s, quote: 'USD', base_asset: assets[s], quote_asset: assets['USD'], **sizes)
    end
    key = ApiKey.new(user:, exchange: alpaca, key: 'k', secret: 's', status: :correct, key_type: :trading, last_synced_at: NOW - 1.hour)
    key.save!(validate: false)
    # The ECB table is current, so Tax::EcbFxRates.ensure_loaded! fetches nothing.
    FxRate.create!(currency: 'USD', date: Date.new(2026, 9, 30), rate: '1.17')
    { user:, alpaca:, assets:, key: }
  end

  def row(ctx, type, base, amount, at, **attrs)
    AccountTransaction.insert!({ user_id: ctx[:user].id, exchange_id: (attrs.delete(:exchange) || ctx[:alpaca]).id, api_key_id: ctx[:key].id,
                                 entry_type: ENTRY.fetch(type.to_s), base_currency: base, base_amount: amount,
                                 transacted_at: Time.iso8601(at), raw_data: {}, manual_values: {},
                                 tx_id: attrs.key?(:tx_id) ? attrs.delete(:tx_id) : "#{type}-#{base}-#{at}",
                                 created_at: Time.current, updated_at: Time.current }.merge(attrs))
  end

  def buy(ctx, symbol, qty, total, at, **attrs) = row(ctx, :buy, symbol, qty, at, quote_currency: 'USD', quote_amount: total, **attrs)
  def sell(ctx, symbol, qty, total, at, **attrs) = row(ctx, :sell, symbol, qty, at, quote_currency: 'USD', quote_amount: total, **attrs)
  def price(symbol, day, value, currency = 'USD') = HistoricalPrice.create!(asset: symbol, currency:, date: Date.parse(day), price: value)

  # A crypto venue of the user's, created once per scenario. No key: no sync banner.
  def venue(ctx, kind)
    ctx[:venues] ||= {}
    ctx[:venues][kind] ||= case kind
                           when :kraken then Exchanges::Kraken.create!(name: 'Kraken', maker_fee: '0.25', taker_fee: '0.4')
                           when :binance then Exchanges::Binance.create!(name: 'Binance', maker_fee: '0.1', taker_fee: '0.1')
                           end
  end

  # Kraken's ledger: one signed row per asset of a trade, grouped by its refid, no quote; the fee on the cash row.
  def kraken(ctx, side, coin, qty, cash, at, fiat: 'EUR', fee: nil, ref: "K-#{coin}-#{at}")
    ex = venue(ctx, :kraken)
    coin_type, cash_type = side == :buy ? %i[buy sell] : %i[sell buy]
    fees = fee ? { fee_currency: fiat, fee_amount: fee } : {}
    row(ctx, coin_type, coin, qty, at, exchange: ex, group_id: ref, tx_id: "#{ref}-#{coin}")
    row(ctx, cash_type, fiat, cash, at, exchange: ex, group_id: ref, tx_id: "#{ref}-#{fiat}", **fees)
  end

  # A Binance Convert: swap legs with no quote, grouped.
  def swap(ctx, out_coin, out_qty, in_coin, in_qty, at, ref:, exchange: venue(ctx, :binance))
    row(ctx, :swap_out, out_coin, out_qty, at, exchange:, group_id: ref, tx_id: "#{ref}-out-#{out_coin}")
    row(ctx, :swap_in, in_coin, in_qty, at, exchange:, group_id: ref, tx_id: "#{ref}-in-#{in_coin}")
  end

  # A withdrawal linked to the deposit it arrived as, ten minutes later.
  def transfer(ctx, coin, qty, at, from:, to:, received: qty)
    row(ctx, :deposit, coin, received, (Time.iso8601(at) + 600).utc.iso8601, exchange: to)
    deposit = AccountTransaction.order(:id).last
    row(ctx, :withdrawal, coin, qty, at, exchange: from, linked_transaction_id: deposit.id)
  end

  def user_zone(ctx, zone) = ctx[:user].update_columns(time_zone: zone)

  USD_PER_EUR = { 2021 => '1.2', 2022 => '1.05', 2023 => '1.07', 2024 => '1.08', 2025 => '1.1', 2026 => '1.17' }.freeze
  # A12-3: the ECB's rate of the report currency where Rails converts into it (CZ: CZK prices are EUR × 25; BG: the peg).
  ECB_EXTRA = { 'CZ' => %w[CZK 25.0], 'BG' => %w[BGN 1.95583] }.freeze
  # The ECB's rate for every day a row was booked on, but the gaps.
  def ecb(gaps, country = nil)
    days = AccountTransaction.pluck(:transacted_at).map { |t| t.utc.to_date }.uniq - gaps.map { |d| Date.parse(d) }
    extra = ECB_EXTRA[country]
    rows = days.flat_map do |d|
      [{ currency: 'USD', date: d, rate: USD_PER_EUR.fetch(d.year, '1.1') }, { currency: 'PLN', date: d, rate: '4.3' }] +
        (extra ? [{ currency: extra[0], date: d, rate: extra[1] }] : [])
    end
    FxRate.insert_all(rows, unique_by: %i[currency date]) if rows.any?
  end

  def scenarios = all_scenarios.map { |s| s.merge(expect: AT_EXPECT.merge(PL_EXPECT).merge(BG_EXPECT).merge(CZ_EXPECT).merge(DK_EXPECT).merge(GR_EXPECT).merge(IT_EXPECT).merge(IE_EXPECT).fetch(s[:name], s[:expect])) }

  def all_scenarios
    [
      { name: 'empty', expect: 'parity' },
      { name: 'quoted_trades', expect: 'parity', setup: lambda { |c|
        row(c, :deposit, 'USD', '5000', '2025-01-02T10:00:00Z')
        buy(c, 'BTC', '3', '100', '2025-01-05T10:00:00Z')
        buy(c, 'BTC', '0.7', '70000.07', '2025-06-01T12:30:00.123456Z')
        sell(c, 'BTC', '1', '50', '2025-11-01T09:00:00Z')
        sell(c, 'BTC', '1.5', '95000.5', '2026-02-03T15:45:10Z')
        buy(c, 'ETH', '2', '6000', '2026-03-01T10:00:00Z')
        sell(c, 'ETH', '0.5', '1000', '2026-04-01T10:00:00Z')
        sell(c, 'ETH', '1.5', '6000', '2026-04-01T10:00:00Z', tx_id: 'eth-sell-2')
        buy(c, 'AAPL', '2', '400', '2026-03-02T15:00:00Z')
        sell(c, 'AAPL', '2', '420', '2026-05-02T15:00:00Z')
        sell(c, 'BTC', '0.2', '20000', '2027-01-01T00:00:00Z', tx_id: nil)
      } },
      { name: 'long_term', expect: 'parity', setup: lambda { |c|
        buy(c, 'SOL', '10', '200', '2024-03-01T10:00:00Z')
        sell(c, 'SOL', '4', '800.4', '2026-03-05T10:00:00Z')
        sell(c, 'SOL', '1', '150', '2026-12-31T23:59:59Z')
      } },
      { name: 'income_deposit_stated', expect: 'parity', setup: lambda { |c|
        price('ETH', '2026-02-01', '3100.25')
        price('BTC', '2026-02-10', '98000')
        row(c, :staking_reward, 'ETH', '0.01', '2026-02-01T00:00:00Z')
        row(c, :airdrop, 'SOL', '3', '2026-02-05T00:00:00Z', manual_values: { 'price' => '150.5' })
        row(c, :deposit, 'BTC', '0.1', '2026-02-10T08:00:00Z')
        sell(c, 'BTC', '0.05', '5000', '2026-03-10T08:00:00Z')
        sell(c, 'SOL', '3', '600', '2026-04-10T08:00:00Z')
      } },
      # No price on 6 January, which no row needs: Rails' range fetch fails and every valuation is still there.
      { name: 'sparse_prices', expect: 'parity', setup: lambda { |c|
        price('ETH', '2026-01-05', '100')
        price('ETH', '2026-01-07', '200')
        row(c, :staking_reward, 'ETH', '1', '2026-01-05T00:00:00Z')
        row(c, :staking_reward, 'ETH', '1', '2026-01-07T00:00:00Z')
        sell(c, 'ETH', '2', '500', '2026-01-08T10:00:00Z')
      } },
      { name: 'sync_banners_and_escaping', expect: 'parity', setup: lambda { |c|
        c[:key].update_columns(last_sync_error: 'boom')
        ApiKey.new(user: c[:user], exchange: c[:alpaca], key: 'r', secret: 's', status: :correct, key_type: :read_only).save!(validate: false)
        ApiKey.new(user: c[:user], exchange: c[:alpaca], key: 'w', secret: 's', status: :correct, key_type: :withdrawal).save!(validate: false)
        buy(c, 'BTC', '1', '100', '2026-01-05T10:00:00Z')
        sell(c, 'BTC', '0.25', '10', '2026-01-06T10:00:00Z', tx_id: '-1,"x"')
        sell(c, 'BTC', '0.25', '10', '2026-01-07T10:00:00Z', tx_id: '=SUM(A1)')
      } },
      { name: 'nul_before_a_formula', expect: 'parity', setup: lambda { |c|
        buy(c, 'BTC', '1', '100', '2026-01-05T10:00:00Z')
        sell(c, 'BTC', '1', '120', '2026-01-06T10:00:00Z', tx_id: 'nul')
        # Bound, not inlined: insert! writes literal SQL, which a NUL cuts short.
        ActiveRecord::Base.connection.exec_update('UPDATE account_transactions SET tx_id = ? WHERE tx_id = ?', 'nul', ["\0=1+1", 'nul'])
      } },
      { name: 'user_classified_and_stock_symbols', expect: 'parity', setup: lambda { |c|
        FundClassification.new(user: c[:user], symbol: 'SOL', kind: :share).save!(validate: false)
        buy(c, 'SOL', '1', '100', '2026-01-05T10:00:00Z')
        sell(c, 'SOL', '1', '120', '2026-01-06T10:00:00Z')
        buy(c, 'BTC', '1', '100', '2026-01-05T10:00:00Z')
        sell(c, 'BTC', '1', '120', '2026-01-06T10:00:00Z')
      } },
      { name: 'tokenized', expect: 'parity', setup: lambda { |c|
        coin = Asset.create!(external_id: 'tslax-coin', symbol: 'TSLAX', name: 'coin', category: 'Cryptocurrency')
        wrapper = Asset.create!(external_id: 'tesla-xstock', symbol: 'TSLAx', name: 'wrapper', category: 'Cryptocurrency', instrument_type: 'tokenized')
        ExchangeAsset.create!(exchange: c[:alpaca], asset: wrapper, available: true)
        Ticker.create!(exchange: c[:alpaca], ticker: 'TSLAX/USD', base: 'TSLAX', quote: 'USD', base_asset: wrapper, quote_asset: c[:assets]['USD'],
                       base_decimals: 9, quote_decimals: 2, price_decimals: 2, minimum_base_size: '0.000000001', minimum_quote_size: '1')
        coin.touch
        buy(c, 'TSLAX', '1', '300', '2026-01-05T10:00:00Z')
      } },
      { name: 'mixed_term_sale', expect: 'parity', setup: lambda { |c|
        buy(c, 'BTC', '1', '30000', '2024-06-01T10:00:00Z')
        buy(c, 'BTC', '0.5', '45000', '2025-01-01T10:00:00Z')
        buy(c, 'BTC', '1', '90000', '2026-01-01T10:00:00Z')
        sell(c, 'BTC', '2.5', '200000.01', '2026-08-01T10:00:00Z')
      } },
      { name: 'leap_anniversary', year: 2025, expect: 'parity', setup: lambda { |c|
        buy(c, 'BTC', '1', '40000', '2024-02-10T15:00:00Z')
        sell(c, 'BTC', '1', '90000', '2025-02-10T16:00:00Z')
      } },
      { name: 'truncated_day', expect: 'parity', setup: lambda { |c|
        buy(c, 'BTC', '1', '40000', '2025-01-10T15:00:00Z')
        sell(c, 'BTC', '1', '90000', '2026-01-11T10:00:00Z')
      } },
      { name: 'local_year_boundary', year: 2025, expect: 'parity', setup: lambda { |c|
        c[:user].update_columns(time_zone: 'Eastern Time (US & Canada)')
        buy(c, 'BTC', '1', '40000', '2025-03-10T15:00:00Z')
        sell(c, 'BTC', '1', '90000', '2026-01-01T03:00:00Z')
      } },
      { name: 'missing_price', expect: 'parity', setup: lambda { |c|
        row(c, :deposit, 'BTC', '0.1', '2026-02-10T08:00:00Z')
        sell(c, 'BTC', '0.1', '9000', '2026-03-10T08:00:00Z')
      } },
      { name: 'oversold', expect: 'diverges: D-TAX-UNCOVERED', setup: lambda { |c|
        buy(c, 'BTC', '1', '100', '2026-01-05T10:00:00Z')
        sell(c, 'BTC', '2', '300', '2026-01-06T10:00:00Z')
      } },
      { name: 'never_held', expect: 'diverges: D-TAX-UNCOVERED', setup: lambda { |c|
        sell(c, 'ETH', '1', '300', '2026-01-06T10:00:00Z')
      } },
      { name: 'cfee_into_buys', expect: 'parity', setup: lambda { |c|
        price('BTC', '2026-01-05', '95000')
        buy(c, 'BTC', '1', '100', '2026-01-05T10:00:00Z')
        buy(c, 'BTC', '3', '330', '2026-01-05T15:00:00Z')
        row(c, :fee, 'BTC', '0.0025', '2026-01-05T00:00:00Z', description: 'CFEE', raw_data: { 'activity_type' => 'CFEE' })
        sell(c, 'BTC', '0.9975', '300', '2026-01-06T10:00:00Z')
      } },
      { name: 'cfee_into_a_sale', expect: 'parity', setup: lambda { |c|
        price('BTC', '2026-01-06', '95000')
        buy(c, 'BTC', '1', '100', '2026-01-05T10:00:00Z')
        sell(c, 'BTC', '0.5', '300', '2026-01-06T10:00:00Z')
        row(c, :fee, 'BTC', '0.001', '2026-01-06T00:00:00Z', description: 'CFEE', raw_data: { 'activity_type' => 'CFEE' })
      } },
      { name: 'feb29_anniversary', year: 2025, expect: 'parity', setup: lambda { |c|
        buy(c, 'BTC', '1', '40000', '2024-02-29T15:00:00Z')
        buy(c, 'BTC', '1', '40000', '2024-03-01T15:00:00Z')
        sell(c, 'BTC', '1', '90000', '2025-03-01T16:00:00Z')
        sell(c, 'BTC', '1', '90000', '2025-03-01T17:00:00Z')
      } },
      # A fill at 02:00 UTC is on 6 January in UTC and on 5 January in New York: the calendars disagree, Rails leaves the fee
      # as a row of its own, consumed at zero proceeds, and names it.
      { name: 'cfee_calendars_disagree', expect: 'diverges: D-TAX-COIN-FEE', setup: lambda { |c|
        price('BTC', '2026-01-05', '95000')
        buy(c, 'BTC', '1', '100', '2026-01-05T10:00:00Z')
        buy(c, 'BTC', '3', '330', '2026-01-06T02:00:00Z')
        row(c, :fee, 'BTC', '0.0025', '2026-01-05T00:00:00Z', description: 'CFEE', raw_data: { 'activity_type' => 'CFEE' })
        sell(c, 'BTC', '0.9975', '300', '2026-01-06T10:00:00Z')
      } },
      { name: 'fee_of_no_trade', expect: 'diverges: D-TAX-COIN-FEE', setup: lambda { |c|
        price('BTC', '2026-01-09', '95000')
        buy(c, 'BTC', '1', '100', '2026-01-05T10:00:00Z')
        row(c, :fee, 'BTC', '0.0025', '2026-01-09T00:00:00Z', description: 'CFEE', raw_data: { 'activity_type' => 'CFEE' })
        sell(c, 'BTC', '0.9975', '300', '2026-01-10T10:00:00Z')
      } },
      { name: 'unlinked_withdrawal', expect: 'diverges: D-TAX-UNLINKED-DISCLOSED', setup: lambda { |c|
        price('BTC', '2026-01-06', '95000')
        buy(c, 'BTC', '1', '100', '2026-01-05T10:00:00Z')
        row(c, :withdrawal, 'BTC', '0.5', '2026-01-06T10:00:00Z')
        sell(c, 'BTC', '0.5', '300', '2026-01-07T10:00:00Z')
      } },
      { name: 'lost', expect: 'diverges: D-TAX-LOST', setup: lambda { |c|
        price('BTC', '2026-01-06', '95000')
        buy(c, 'BTC', '1', '100', '2026-01-05T10:00:00Z')
        row(c, :lost, 'BTC', '0.5', '2026-01-06T10:00:00Z')
        sell(c, 'BTC', '0.5', '300', '2026-01-07T10:00:00Z')
      } },
      { name: 'unsupported', expect: 'parity', setup: lambda { |c|
        buy(c, 'BTC', '1', '100', '2026-01-05T10:00:00Z')
        row(c, :unsupported_activity, 'BTC', '1', '2026-01-06T10:00:00Z')
      } },
      { name: 'other_venue', expect: 'parity', setup: lambda { |c|
        kraken = Exchanges::Kraken.create!(name: 'Kraken', maker_fee: '0.25', taker_fee: '0.4')
        buy(c, 'BTC', '1', '100', '2026-01-05T10:00:00Z', exchange: kraken)
      } }
    ] + us_more + fifo_jurisdictions + rails_cases + at_scenarios + pl_scenarios + bg_scenarios + cz_scenarios + dk_scenarios + gr_scenarios + it_scenarios + ie_scenarios
  end

  # A12-2: US beyond Alpaca, and D-TAX-US-WALLET (lots per venue for tax years from 2025).
  def us_more
    [
      # Before 2025, so one pool: the Binance Convert takes the Kraken lot.
      { name: 'us_kraken_and_binance', year: 2024, expect: 'parity', setup: lambda { |c|
        price('BTC', '2024-01-10', '90000')
        price('ETH', '2024-02-01', '3000')
        price('BTC', '2024-02-01', '100000')
        kraken(c, :buy, 'BTC', '0.5', '45000', '2024-01-10T10:00:00Z', fiat: 'USD', fee: '30')
        swap(c, 'BTC', '0.1', 'ETH', '3.3', '2024-02-01T10:00:00Z', ref: 'cv1')
        kraken(c, :sell, 'BTC', '0.2', '21000', '2024-03-01T10:00:00Z', fiat: 'USD', fee: '12')
      } },
      # From 2025 per venue: Binance holds no BTC of its own, so the Convert is uncovered there.
      { name: 'us_wallet_kraken_and_binance', expect: 'diverges: D-TAX-US-WALLET', setup: lambda { |c|
        price('BTC', '2026-01-10', '90000')
        price('ETH', '2026-02-01', '3000')
        price('BTC', '2026-02-01', '100000')
        kraken(c, :buy, 'BTC', '0.5', '45000', '2026-01-10T10:00:00Z', fiat: 'USD', fee: '30')
        swap(c, 'BTC', '0.1', 'ETH', '3.3', '2026-02-01T10:00:00Z', ref: 'cv1')
        kraken(c, :sell, 'BTC', '0.2', '21000', '2026-03-01T10:00:00Z', fiat: 'USD', fee: '12')
      } },
      # Older lots on Alpaca, a sale on Kraken: per venue, the sale takes Kraken's own lot.
      { name: 'us_wallet_sale_on_other_venue', expect: 'diverges: D-TAX-US-WALLET', setup: lambda { |c|
        kr = venue(c, :kraken)
        buy(c, 'BTC', '1', '30000', '2025-01-05T10:00:00Z')
        buy(c, 'BTC', '1', '90000', '2025-06-01T10:00:00Z', exchange: kr)
        sell(c, 'BTC', '1', '100000', '2026-02-01T10:00:00Z', exchange: kr)
      } },
      # A linked transfer carries the oldest lot to Kraken; the Kraken sale takes it and Kraken's own, not Alpaca's.
      { name: 'us_wallet_linked_transfer', expect: 'diverges: D-TAX-US-WALLET', setup: lambda { |c|
        kr = venue(c, :kraken)
        buy(c, 'BTC', '1', '30000', '2025-01-05T10:00:00Z')
        buy(c, 'BTC', '1', '40000', '2025-01-10T10:00:00Z')
        buy(c, 'BTC', '1', '60000', '2025-02-01T10:00:00Z', exchange: kr)
        price('BTC', '2025-03-01', '85000')
        transfer(c, 'BTC', '1.0005', '2025-03-01T10:00:00Z', from: c[:alpaca], to: kr, received: '1')
        sell(c, 'BTC', '2', '200000', '2026-02-01T10:00:00Z', exchange: kr)
      } },
      # Before 2025 one pool: the 2024 report is Rails'.
      { name: 'us_wallet_2024_pooled', year: 2024, expect: 'parity', setup: ->(c) { wallet_boundary(c) } },
      # The same history for 2025: the pool as it stands at the end of 2024 is placed on the venues holding the units,
      # so no lot the 2024 report used is used again, and the 2025 Kraken sale takes Kraken's 2025 lot.
      { name: 'us_wallet_2025_per_venue', year: 2025, expect: 'diverges: D-TAX-US-WALLET', setup: ->(c) { wallet_boundary(c) } },
      # A sale on Kraken stored before the withdrawal that brought its coins, in the same second: walked after it.
      { name: 'us_wallet_same_second_transfer', expect: 'parity', setup: lambda { |c|
        kr = venue(c, :kraken)
        price('BTC', '2026-01-10', '95000')
        buy(c, 'BTC', '1', '100', '2026-01-05T10:00:00Z')
        sell(c, 'BTC', '1', '200', '2026-01-10T10:00:00Z', exchange: kr)
        row(c, :deposit, 'BTC', '1', '2026-01-10T10:10:00Z', exchange: kr)
        row(c, :withdrawal, 'BTC', '1', '2026-01-10T10:00:00Z', linked_transaction_id: AccountTransaction.order(:id).last.id)
      } },
      # A coin held on two venues at the end of 2024: the pool's oldest lot goes to the venue first seen, and the report
      # lists the placement as its reconstruction.
      { name: 'us_wallet_allocation', year: 2025, expect: 'diverges: D-TAX-US-WALLET', setup: lambda { |c|
        kr = venue(c, :kraken)
        buy(c, 'BTC', '1', '30000', '2023-06-01T10:00:00Z')
        buy(c, 'BTC', '1', '90000', '2024-01-05T10:00:00Z', exchange: kr)
        sell(c, 'BTC', '1', '100000', '2025-03-01T10:00:00Z', exchange: kr)
      } },
      # A BNB fee on a buy: spending BNB is a disposal of BNB at the fee's value.
      { name: 'us_third_coin_fee', year: 2024, expect: 'diverges: D-TAX-COIN-FEE', setup: lambda { |c|
        bn = venue(c, :binance)
        price('BNB', '2024-02-01', '400')
        row(c, :buy, 'BNB', '1', '2024-01-05T10:00:00Z', exchange: bn, quote_currency: 'USD', quote_amount: '300')
        row(c, :buy, 'BTC', '1', '2024-02-01T10:00:00Z', exchange: bn, quote_currency: 'USD', quote_amount: '40000', fee_currency: 'BNB', fee_amount: '0.1')
        row(c, :sell, 'BTC', '1', '2024-03-01T10:00:00Z', exchange: bn, quote_currency: 'USD', quote_amount: '60000')
      } },
      # From 2025 a swap's fee comes off what the coin given up fetched, not onto the new coin's basis.
      { name: 'us_swap_cost_to_disposed', expect: 'diverges: D-TAX-US-SWAP-COST', setup: lambda { |c|
        bn = venue(c, :binance)
        price('ETH', '2026-02-01', '3000')
        price('BTC', '2026-02-01', '100000')
        price('BNB', '2026-02-01', '600')
        row(c, :buy, 'BNB', '1', '2026-01-05T10:00:00Z', exchange: bn, quote_currency: 'USD', quote_amount: '500')
        row(c, :buy, 'ETH', '1', '2026-01-05T11:00:00Z', exchange: bn, quote_currency: 'USD', quote_amount: '2500')
        row(c, :swap_out, 'ETH', '1', '2026-02-01T10:00:00Z', exchange: bn, group_id: 'sc', tx_id: 'sc-o')
        row(c, :swap_in, 'BTC', '0.03', '2026-02-01T10:00:00Z', exchange: bn, group_id: 'sc', tx_id: 'sc-i', fee_currency: 'BNB', fee_amount: '0.01')
        row(c, :sell, 'BTC', '0.03', '2026-03-01T10:00:00Z', exchange: bn, quote_currency: 'USD', quote_amount: '3300')
      } },
      # Two coins under one ticker on two venues: neither takes the other's basis.
      { name: 'us_two_coins_one_symbol', year: 2024, expect: 'diverges: D-TAX-ASSET-IDENTITY', setup: lambda { |c|
        kr = venue(c, :kraken)
        bn = venue(c, :binance)
        a = Asset.create!(external_id: 'the-open-network', symbol: 'TON', name: 'Toncoin', category: 'Cryptocurrency')
        b = Asset.create!(external_id: 'tokamak-network', symbol: 'TON', name: 'Tokamak', category: 'Cryptocurrency')
        [[kr, a], [kr, c[:assets]['USD']], [bn, b], [bn, c[:assets]['USD']]].each { |ex, as| ExchangeAsset.create!(exchange: ex, asset: as, available: true) }
        sizes = { base_decimals: 8, quote_decimals: 2, price_decimals: 2, minimum_base_size: '0.00000001', minimum_quote_size: '1' }
        Ticker.create!(exchange: kr, ticker: 'TON/USD', base: 'TON', quote: 'USD', base_asset: a, quote_asset: c[:assets]['USD'], **sizes)
        Ticker.create!(exchange: bn, ticker: 'TONUSD', base: 'TON', quote: 'USD', base_asset: b, quote_asset: c[:assets]['USD'], **sizes)
        row(c, :buy, 'TON', '10', '2024-01-05T10:00:00Z', exchange: bn, quote_currency: 'USD', quote_amount: '20', base_asset_id: b.id)
        row(c, :buy, 'TON', '10', '2024-02-05T10:00:00Z', exchange: kr, quote_currency: 'USD', quote_amount: '50', base_asset_id: a.id)
        row(c, :sell, 'TON', '10', '2024-03-05T10:00:00Z', exchange: kr, quote_currency: 'USD', quote_amount: '70', base_asset_id: a.id)
      } },
      # A sale's fee in the coin sold leaves on top of the amount sold: a disposal of those units too.
      { name: 'us_sale_fee_in_sold_coin', year: 2024, expect: 'diverges: D-TAX-COIN-FEE', setup: lambda { |c|
        bn = venue(c, :binance)
        price('BTC', '2024-03-01', '60000')
        row(c, :buy, 'BTC', '2', '2024-01-05T10:00:00Z', exchange: bn, quote_currency: 'USD', quote_amount: '80000')
        row(c, :sell, 'BTC', '1', '2024-03-01T10:00:00Z', exchange: bn, quote_currency: 'USD', quote_amount: '60000', fee_currency: 'BTC', fee_amount: '0.01')
        row(c, :sell, 'BTC', '0.99', '2024-04-01T10:00:00Z', exchange: bn, quote_currency: 'USD', quote_amount: '59400')
      } },
      # From 2025, a fee taken from the coin received: the net units keep what is left of the value, the fee comes off
      # the coin given up; and a BTC-for-USDT trade Kraken books as a sell and a buy is such an exchange too.
      { name: 'us_swap_cost_in_received_coin', expect: 'diverges: D-TAX-US-SWAP-COST', setup: lambda { |c|
        bn = venue(c, :binance)
        price('ETH', '2026-02-01', '3000')
        price('BTC', '2026-02-01', '100000')
        row(c, :buy, 'ETH', '1', '2026-01-05T11:00:00Z', exchange: bn, quote_currency: 'USD', quote_amount: '2500')
        row(c, :swap_out, 'ETH', '1', '2026-02-01T10:00:00Z', exchange: bn, group_id: 'rc', tx_id: 'rc-o')
        row(c, :swap_in, 'BTC', '0.03', '2026-02-01T10:00:00Z', exchange: bn, group_id: 'rc', tx_id: 'rc-i', fee_currency: 'BTC', fee_amount: '0.0003')
        row(c, :sell, 'BTC', '0.0297', '2026-03-01T10:00:00Z', exchange: bn, quote_currency: 'USD', quote_amount: '3300')
        kr = venue(c, :kraken)
        row(c, :buy, 'SOL', '10', '2026-01-06T10:00:00Z', exchange: kr, quote_currency: 'USD', quote_amount: '1000')
        row(c, :sell, 'SOL', '10', '2026-02-02T10:00:00Z', exchange: kr, group_id: 'ku', tx_id: 'ku-sol')
        row(c, :buy, 'USDT', '1500', '2026-02-02T10:00:00Z', exchange: kr, group_id: 'ku', tx_id: 'ku-usdt', fee_currency: 'USDT', fee_amount: '3')
        row(c, :sell, 'USDT', '1497', '2026-03-02T10:00:00Z', exchange: kr, quote_currency: 'USD', quote_amount: '1497')
      } },
      # A 2025 report of a history that stops in 2024, held on two venues: the placement is listed all the same.
      { name: 'us_wallet_allocation_no_2025_rows', year: 2025, expect: 'diverges: D-TAX-US-WALLET', setup: lambda { |c|
        buy(c, 'BTC', '1', '30000', '2023-06-01T10:00:00Z')
        buy(c, 'BTC', '1', '90000', '2024-01-05T10:00:00Z', exchange: venue(c, :kraken))
      } },
      # A New York taxpayer's CFEE at 00:00 UTC on 1 January (31 December there): only that local year's fills order it,
      # so the 2024 and 2025 reports agree on its lots.
      { name: 'us_cfee_new_york_year_end', year: 2024, expect: 'diverges: D-TAX-COIN-FEE', setup: lambda { |c|
        user_zone(c, 'Eastern Time (US & Canada)')
        price('ETH', '2025-01-01', '3000')
        buy(c, 'ETH', '0.05', '50', '2025-01-01T02:00:00Z')
        buy(c, 'ETH', '1', '2000', '2025-01-01T12:00:00Z')
        buy(c, 'ETH', '0.5', '900', '2024-12-31T03:00:00Z')
        row(c, :fee, 'ETH', '0.1', '2025-01-01T00:00:00Z', description: 'CFEE', raw_data: { 'activity_type' => 'CFEE' })
      } },
      # A fiat-funded buy grouped with a coin sale: not a coin-for-coin exchange, so the buy's fee stays its own.
      { name: 'us_swap_cost_not_with_fiat', expect: 'parity', setup: lambda { |c|
        kr = venue(c, :kraken)
        price('SOL', '2026-02-02', '150')
        row(c, :buy, 'SOL', '10', '2026-01-06T10:00:00Z', exchange: kr, quote_currency: 'USD', quote_amount: '1000')
        row(c, :sell, 'SOL', '10', '2026-02-02T10:00:00Z', exchange: kr, group_id: 'mx', tx_id: 'mx-sol')
        row(c, :sell, 'USD', '500', '2026-02-02T10:00:00Z', exchange: kr, group_id: 'mx', tx_id: 'mx-usd')
        row(c, :buy, 'ETH', '0.6', '2026-02-02T10:00:00Z', exchange: kr, group_id: 'mx', tx_id: 'mx-eth', fee_currency: 'USD', fee_amount: '5')
      } },
      # A coin-for-coin trade Bybit books as one sell of BTC quoted in ETH: the ETH it brought is held, at its value then.
      { name: 'us_coin_quoted_pair', year: 2024, expect: 'diverges: D-TAX-COIN-QUOTE', setup: lambda { |c|
        by = Exchanges::Bybit.create!(name: 'Bybit', maker_fee: '0.1', taker_fee: '0.1')
        price('BTC', '2024-02-01', '48000')
        price('ETH', '2024-02-01', '3000')
        row(c, :buy, 'USDT', '30000', '2024-01-02T10:00:00Z', exchange: by, quote_currency: 'USD', quote_amount: '30000')
        row(c, :buy, 'BTC', '1', '2024-01-05T10:00:00Z', exchange: by, quote_currency: 'USDT', quote_amount: '30000')
        row(c, :sell, 'BTC', '0.5', '2024-02-01T10:00:00Z', exchange: by, quote_currency: 'ETH', quote_amount: '8', fee_currency: 'ETH', fee_amount: '0.01')
        row(c, :sell, 'ETH', '7.99', '2024-03-01T10:00:00Z', exchange: by, quote_currency: 'USDT', quote_amount: '28000')
      } },
      # BTC sold for USDT in one row: the USDT received is held at its value then, so selling it later gains nothing.
      { name: 'us_stable_quoted_sale', expect: 'diverges: D-TAX-COIN-QUOTE', setup: lambda { |c|
        by = Exchanges::Bybit.create!(name: 'Bybit', maker_fee: '0.1', taker_fee: '0.1')
        row(c, :buy, 'BTC', '1', '2026-01-05T10:00:00Z', exchange: by, quote_currency: 'USD', quote_amount: '30000')
        row(c, :sell, 'BTC', '1', '2026-02-01T10:00:00Z', exchange: by, quote_currency: 'USDT', quote_amount: '60000')
        row(c, :sell, 'USDT', '60000', '2026-03-01T10:00:00Z', exchange: by, quote_currency: 'USD', quote_amount: '60000')
      } },
      # A buy and a coin-quoted sale of the same BTC in one second: the buy's lot is there for the sale.
      { name: 'us_coin_quoted_same_second', expect: 'parity', setup: lambda { |c|
        by = Exchanges::Bybit.create!(name: 'Bybit', maker_fee: '0.1', taker_fee: '0.1')
        price('BTC', '2026-02-01', '50000')
        price('ETH', '2026-02-01', '2500')
        row(c, :buy, 'BTC', '1', '2026-02-01T10:00:00Z', exchange: by, quote_currency: 'USD', quote_amount: '40000')
        row(c, :sell, 'BTC', '1', '2026-02-01T10:00:00Z', exchange: by, quote_currency: 'ETH', quote_amount: '20')
      } },
      # A withdrawal's fee in another coin is spent: 0.1 BNB worth $60 against $50 of basis.
      { name: 'us_withdrawal_fee_third_coin', expect: 'diverges: D-TAX-COIN-FEE', setup: lambda { |c|
        bn = venue(c, :binance)
        price('BNB', '2026-02-01', '600')
        price('ETH', '2026-02-01', '3100')
        buy(c, 'BNB', '1', '500', '2026-01-05T10:00:00Z', exchange: bn)
        buy(c, 'ETH', '1', '3000', '2026-01-06T10:00:00Z', exchange: bn)
        row(c, :withdrawal, 'ETH', '1', '2026-02-01T10:00:00Z', exchange: bn, fee_currency: 'BNB', fee_amount: '0.1')
        sell(c, 'BNB', '0.9', '630', '2026-03-01T10:00:00Z', exchange: bn)
      } },
      { name: 'us_unlinked_withdrawal_per_venue', expect: 'diverges: D-TAX-US-WALLET', setup: lambda { |c|
        price('BTC', '2026-01-06', '95000')
        buy(c, 'BTC', '1', '100', '2026-01-05T10:00:00Z')
        buy(c, 'BTC', '1', '200', '2026-01-05T11:00:00Z')
        row(c, :withdrawal, 'BTC', '1', '2026-01-06T10:00:00Z')
        sell(c, 'BTC', '1', '300', '2026-01-07T10:00:00Z')
      } },
      { name: 'us_coin_fee_other_venue', expect: 'diverges: D-TAX-COIN-FEE', setup: lambda { |c|
        price('BNB', '2026-01-05', '600')
        price('BNB', '2026-02-01', '700')
        bn = venue(c, :binance)
        buy(c, 'BNB', '1', '600', '2026-01-05T10:00:00Z', exchange: bn)
        row(c, :fee, 'BNB', '0.1', '2026-02-01T10:00:00Z', exchange: bn)
      } },
      { name: 'us_lost_then_sold', expect: 'diverges: D-TAX-LOST', setup: lambda { |c|
        price('BTC', '2026-01-06', '95000')
        buy(c, 'BTC', '1', '100', '2026-01-05T10:00:00Z')
        buy(c, 'BTC', '1', '500', '2026-01-05T11:00:00Z')
        row(c, :lost, 'BTC', '1', '2026-01-06T10:00:00Z')
        sell(c, 'BTC', '1', '700', '2026-01-07T10:00:00Z')
      } }
    ]
  end

  def wallet_boundary(c)
    kr = venue(c, :kraken)
    buy(c, 'BTC', '1', '30000', '2023-06-01T10:00:00Z')
    buy(c, 'BTC', '1', '90000', '2024-01-05T10:00:00Z', exchange: kr)
    sell(c, 'BTC', '1', '100000', '2024-03-01T10:00:00Z', exchange: kr)
    buy(c, 'BTC', '1', '50000', '2025-02-01T10:00:00Z', exchange: kr)
    sell(c, 'BTC', '1', '80000', '2025-03-01T10:00:00Z', exchange: kr)
  end

  # The FIFO jurisdictions without a summary: DE AT ES PT PL SK.
  # A12-2b: Austria's own scenarios and labels (the shared scenarios' labels by name).
  # The shared scenarios Austria's own law moves away from Rails' one FIFO pool.
  AT_EXPECT = {
    'at_buy_sell_income_deposit_stated' => 'diverges: D-TAX-AT-REWARDS',
    'at_stablecoin_as_fiat' => 'diverges: D-TAX-AT-STABLECOIN',
    'at_lost' => 'diverges: D-TAX-AT-AVERAGE',
    'at_coin_fee' => 'diverges: D-TAX-AT-FEES',
    'at_old_stock' => 'diverges: D-TAX-AT-AVERAGE',
    'at_old_stock_held_short' => 'diverges: D-TAX-AT-SPECULATION',
    'at_sweep_keeps_ages' => 'diverges: D-TAX-AT-OLD-SWAP',
    'at_sweep_several_coins' => 'diverges: D-TAX-AT-AVERAGE',
    'at_swap_fills_and_values' => 'diverges: D-TAX-AT-AVERAGE',
    'at_unweighable_and_uncovered_legs' => 'diverges: D-TAX-AT-AVERAGE',
    'at_swap_in_fees' => 'diverges: D-TAX-AT-FEES',
    'at_mixed_cash_and_coin' => 'diverges: D-TAX-AT-AVERAGE',
    'at_older_tranche_and_unpriced' => 'diverges: D-TAX-AT-AVERAGE'
  }.freeze
  # Every figure is the plan's (PLAN-A12-tax-reports.md, A12-2b, Vectors), computed by hand; Vienna, euros.
  def at_scenarios
    rz6178aa = lambda do |c, old: false|
      user_zone(c, 'Vienna')
      kraken(c, :buy, 'ETH', '5', '10', '2019-06-01T10:00:00Z', ref: 'b0') if old
      kraken(c, :buy, 'ETH', '1', '15', '2021-03-05T10:00:00Z', ref: 'b1')
      kraken(c, :buy, 'ETH', '2', '60', '2021-05-05T10:00:00Z', ref: 'b2')
      kraken(c, :buy, 'ETH', '5', '10', '2022-06-19T10:00:00Z', ref: 'b3')
      kraken(c, :sell, 'ETH', '1', '20', '2022-10-10T10:00:00Z', ref: 's1')
      kraken(c, :sell, 'ETH', '2', '30', '2023-01-03T10:00:00Z', ref: 's2')
    end
    # Two network-fee scenarios share the buys: average 80,080 / 2 = 40,040.
    two_btc_buys = lambda do |c|
      user_zone(c, 'Vienna')
      kraken(c, :buy, 'BTC', '1', '30000', '2025-01-05T10:00:00Z', fee: '30', ref: 'b1')
      kraken(c, :buy, 'BTC', '1', '50000', '2025-02-05T10:00:00Z', fee: '50', ref: 'b2')
      price('BTC', '2026-02-01', '100000', 'EUR')
    end
    # Lending of 0.01 ETH on 5, 12, 19 (and 26) March 2026; the 2nd's row is the 1st's close.
    monthly = lambda do |c, days|
      user_zone(c, 'Vienna')
      price('ETH', '2026-03-01', '1900', 'EUR')
      price('ETH', '2026-03-02', '2000', 'EUR')
      days.each { |d| price('ETH', "2026-03-#{d}", '3000', 'EUR') }
      days.each { |d| row(c, :lending_interest, 'ETH', '0.01', "2026-03-#{d}T10:00:00Z", exchange: venue(c, :kraken)) }
      kraken(c, :sell, 'ETH', (0.01 * days.size).round(2).to_s, '150', '2026-04-01T10:00:00Z', ref: 's1')
    end
    old_btc_2022 = lambda do |c|
      # Old stock (bought before 1 March 2021) sold within a year: § 31 speculation, the fee a cost.
      kraken(c, :buy, 'BTC', '1', '1000', '2021-02-10T10:00:00Z', ref: 'b1')
      kraken(c, :sell, 'BTC', '1', '2000', '2022-01-15T10:00:00Z', fee: '20', ref: 's1')
    end
    eth_loss_2022 = lambda do |c|
      kraken(c, :buy, 'ETH', '1', '500', '2022-03-10T10:00:00Z', ref: 'b2')
      kraken(c, :sell, 'ETH', '1', '300', '2022-06-01T10:00:00Z', ref: 's2')
    end
    swap_2021 = lambda do |c|
      # BTC 20,000 (5 Mar 2021); 1 Dec 2021 swapped (worth 30,000) for 10 ETH: a Tausch under § 31, so 2021 holds the
      # BTC disposal 30,000 / 20,000 / 10,000 (old_stock false) and the ETH costs 30,000; ETH sold 2025 for 40,000 →
      # cost 30,000, gain 10,000 (Rails: no 2021 row, 2025 gain 20,000).
      user_zone(c, 'Vienna')
      bn = venue(c, :binance)
      price('BTC', '2021-12-01', '30000', 'EUR')
      # ETH's own price that day (2,900 × 10) differs: the ETH still costs the 30,000 given (§ 6 Z 14).
      price('ETH', '2021-12-01', '2900', 'EUR')
      row(c, :buy, 'BTC', '1', '2021-03-05T10:00:00Z', exchange: bn, quote_currency: 'EUR', quote_amount: '20000')
      swap(c, 'BTC', '1', 'ETH', '10', '2021-12-01T10:00:00Z', ref: 'sw')
      row(c, :sell, 'ETH', '10', '2025-06-01T10:00:00Z', exchange: bn, quote_currency: 'EUR', quote_amount: '40000')
    end
    [
      # Rz 6178aa: 2022 sale FIFO → cost 15, gain 5; 1 Jan 2023: 70 / 7 = 10; 2023 sale of 2 for 30 → cost 20, gain 10.
      { name: 'at_average_rz6178aa_2022', country: 'AT', year: 2022, expect: 'parity', setup: ->(c) { rz6178aa.(c) } },
      { name: 'at_average_rz6178aa_2023', country: 'AT', year: 2023, expect: 'diverges: D-TAX-AT-AVERAGE', setup: ->(c) { rz6178aa.(c) } },
      # Plus 5 old units (2019, 10): the 2022 sale takes an old unit (cost 2, gain 18, old_stock true); the 2023 sale of 2
      # takes old units (cost 4, gain 26, old_stock true): old first (VO § 3 Z 2).
      { name: 'at_average_old_first_2022', country: 'AT', year: 2022, expect: 'parity', setup: ->(c) { rz6178aa.(c, old: true) } },
      { name: 'at_average_old_first', country: 'AT', year: 2023, expect: 'parity', setup: ->(c) { rz6178aa.(c, old: true) } },
      # 23:30 Vienna on 28 Feb 2021 is old stock, 00:30 on 1 March new: old 60,000 − 40,000 = 20,000 (true); new
      # 60,000 − 41,000 = 19,000 (false).
      { name: 'at_cutoff_edge', country: 'AT', expect: 'parity', setup: lambda { |c|
        user_zone(c, 'Vienna')
        kraken(c, :buy, 'BTC', '1', '40000', '2021-02-28T22:30:00Z', ref: 'b1')
        kraken(c, :buy, 'BTC', '1', '41000', '2021-02-28T23:30:00Z', ref: 'b2')
        kraken(c, :sell, 'BTC', '2', '120000', '2026-03-02T10:00:00Z', ref: 's1')
      } },
      # Old row 100,000 / 10,000 / 90,000 true (fee 100); new row 100,000 / 50,000 / 50,000 false (fee 100, not deducted).
      { name: 'at_mixed_sale_with_fee', country: 'AT', expect: 'diverges: D-TAX-AT-SALE-FEE', setup: lambda { |c|
        user_zone(c, 'Vienna')
        kraken(c, :buy, 'BTC', '1', '10000', '2020-06-01T10:00:00Z', ref: 'b1')
        kraken(c, :buy, 'BTC', '1', '50000', '2024-06-01T10:00:00Z', ref: 'b2')
        kraken(c, :sell, 'BTC', '2', '200000', '2026-03-02T10:00:00Z', fee: '200', ref: 's1')
      } },
      # Bybit's ETH (30) is older than Kraken's (10); the Kraken sale for 40 takes Kraken's: cost 10, gain 30 (one pool:
      # 30 / 10; one global average: 20 / 20).
      { name: 'at_average_per_wallet', country: 'AT', expect: 'diverges: D-TAX-AT-WALLET', setup: lambda { |c|
        user_zone(c, 'Vienna')
        row(c, :buy, 'ETH', '1', '2024-01-05T10:00:00Z', exchange: at_bybit(c), quote_currency: 'EUR', quote_amount: '30')
        kraken(c, :buy, 'ETH', '1', '10', '2024-02-05T10:00:00Z', ref: 'b1')
        kraken(c, :sell, 'ETH', '1', '40', '2026-03-02T10:00:00Z', ref: 's1')
      } },
      # Kraken 1 @ 10; 1 @ 30 arrives from Bybit: 2 @ 40, the sale of 1 for 40 → cost 20, gain 20.
      { name: 'at_transfer_into_average', country: 'AT', expect: 'diverges: D-TAX-AT-AVERAGE', setup: lambda { |c|
        user_zone(c, 'Vienna')
        kraken(c, :buy, 'ETH', '1', '10', '2024-01-05T10:00:00Z', ref: 'b1')
        row(c, :buy, 'ETH', '1', '2024-02-05T10:00:00Z', exchange: at_bybit(c), quote_currency: 'EUR', quote_amount: '30')
        price('ETH', '2025-03-01', '20', 'EUR')
        transfer(c, 'ETH', '1', '2025-03-01T10:00:00Z', from: at_bybit(c), to: venue(c, :kraken))
        kraken(c, :sell, 'ETH', '1', '40', '2026-03-02T10:00:00Z', ref: 's1')
      } },
      # 2 for 100, 1 sold (cost 50), 1 bought for 80: 2 @ 130, average 65; 1 sold for 100 → cost 65, gain 35.
      { name: 'at_buy_after_partial_sale', country: 'AT', expect: 'diverges: D-TAX-AT-AVERAGE', setup: lambda { |c|
        user_zone(c, 'Vienna')
        kraken(c, :buy, 'ETH', '2', '100', '2024-03-01T10:00:00Z', ref: 'b1')
        kraken(c, :sell, 'ETH', '1', '70', '2025-03-01T10:00:00Z', ref: 's1')
        kraken(c, :buy, 'ETH', '1', '80', '2025-04-01T10:00:00Z', ref: 'b2')
        kraken(c, :sell, 'ETH', '1', '100', '2026-03-02T10:00:00Z', ref: 's2')
      } },
      # Staking 0.1 ETH (worth 300) at cost zero, no income row; sold for 400 → cost 0, gain 400. Lending 1 SOL → income 15.
      { name: 'at_rewards_zero_cost', country: 'AT', expect: 'diverges: D-TAX-AT-REWARDS', setup: lambda { |c|
        user_zone(c, 'Vienna')
        price('ETH', '2026-02-01', '3000', 'EUR')
        price('SOL', '2026-02-10', '15', 'EUR')
        row(c, :staking_reward, 'ETH', '0.1', '2026-02-01T10:00:00Z', exchange: venue(c, :kraken))
        row(c, :lending_interest, 'SOL', '1', '2026-02-10T10:00:00Z', exchange: venue(c, :kraken))
        kraken(c, :sell, 'ETH', '0.1', '400', '2026-03-01T10:00:00Z', ref: 's1')
      } },
      # 1 ETH for 2,000 and 0.1 ETH staked at zero: 1.1 @ 2,000; 0.55 sold for 1,500 → cost 1,000, gain 500.
      { name: 'at_reward_joins_pool', country: 'AT', expect: 'diverges: D-TAX-AT-REWARDS', setup: lambda { |c|
        user_zone(c, 'Vienna')
        price('ETH', '2025-02-01', '3000', 'EUR')
        kraken(c, :buy, 'ETH', '1', '2000', '2024-03-01T10:00:00Z', ref: 'b1')
        row(c, :staking_reward, 'ETH', '0.1', '2025-02-01T10:00:00Z', exchange: venue(c, :kraken))
        kraken(c, :sell, 'ETH', '0.55', '1500', '2026-03-02T10:00:00Z', ref: 's1')
      } },
      # Rz 6178ad: 10 SOL for 1,000; 5 SOL + 0.1 SOL fee swapped for 1 ETH → ETH 500, 4.9 SOL at 500. ETH sold for 2,000
      # → cost 500, gain 1,500; 4.9 SOL sold for 3,000 → cost 500, gain 2,500.
      { name: 'at_swap_fee_ignored', country: 'AT', expect: 'diverges: D-TAX-AT-FEES', setup: lambda { |c|
        user_zone(c, 'Vienna')
        bn = venue(c, :binance)
        price('SOL', '2025-06-01', '100', 'EUR')
        price('ETH', '2025-06-01', '500', 'EUR')
        row(c, :buy, 'SOL', '10', '2024-06-01T10:00:00Z', exchange: bn, quote_currency: 'EUR', quote_amount: '1000')
        row(c, :swap_out, 'SOL', '5', '2025-06-01T10:00:00Z', exchange: bn, group_id: 'fz', tx_id: 'fz-o', fee_currency: 'SOL', fee_amount: '0.1')
        row(c, :swap_in, 'ETH', '1', '2025-06-01T10:00:00Z', exchange: bn, group_id: 'fz', tx_id: 'fz-i')
        row(c, :sell, 'ETH', '1', '2026-03-02T10:00:00Z', exchange: bn, quote_currency: 'EUR', quote_amount: '2000')
        row(c, :sell, 'SOL', '4.9', '2026-03-03T10:00:00Z', exchange: bn, quote_currency: 'EUR', quote_amount: '3000')
      } },
      # Old BTC (2020, 10,000) swapped 2024 (worth 50,000) for 20 ETH: new stock at 50,000; sold for 60,000 → cost
      # 50,000, gain 10,000, old_stock false.
      { name: 'at_old_stock_swapped', country: 'AT', expect: 'diverges: D-TAX-AT-OLD-SWAP', setup: lambda { |c|
        user_zone(c, 'Vienna')
        bn = venue(c, :binance)
        price('BTC', '2024-06-01', '50000', 'EUR')
        price('ETH', '2024-06-01', '2500', 'EUR')
        row(c, :buy, 'BTC', '1', '2020-06-01T10:00:00Z', exchange: bn, quote_currency: 'EUR', quote_amount: '10000')
        swap(c, 'BTC', '1', 'ETH', '20', '2024-06-01T10:00:00Z', ref: 'os')
        row(c, :sell, 'ETH', '20', '2026-03-02T10:00:00Z', exchange: bn, quote_currency: 'EUR', quote_amount: '60000')
      } },
      # Old BTC (2020, 10,000) and new BTC (2022, 30,000) swapped 2024 (50,000 each) for 40 ETH: the old half at 50,000,
      # the new at 30,000; 40 ETH sold for 120,000 → cost 80,000, gain 40,000.
      { name: 'at_mixed_old_new_swap', country: 'AT', expect: 'diverges: D-TAX-AT-OLD-SWAP', setup: lambda { |c|
        user_zone(c, 'Vienna')
        bn = venue(c, :binance)
        price('BTC', '2024-06-01', '50000', 'EUR')
        price('ETH', '2024-06-01', '2500', 'EUR')
        row(c, :buy, 'BTC', '1', '2020-06-01T10:00:00Z', exchange: bn, quote_currency: 'EUR', quote_amount: '10000')
        row(c, :buy, 'BTC', '1', '2022-06-01T10:00:00Z', exchange: bn, quote_currency: 'EUR', quote_amount: '30000')
        swap(c, 'BTC', '2', 'ETH', '40', '2024-06-01T10:00:00Z', ref: 'ms')
        row(c, :sell, 'ETH', '40', '2026-03-02T10:00:00Z', exchange: bn, quote_currency: 'EUR', quote_amount: '120000')
      } },
      # A fiat purchase booked as swap legs (EUR out, ETH in, €10 fee): a purchase, its fee part of the cost → 1,010 / 990.
      { name: 'at_fiat_swap_legs_fee', country: 'AT', expect: 'parity', setup: lambda { |c|
        user_zone(c, 'Vienna')
        bn = venue(c, :binance)
        price('ETH', '2026-02-01', '1000', 'EUR')
        row(c, :swap_out, 'EUR', '1000', '2026-02-01T10:00:00Z', exchange: bn, group_id: 'fp', tx_id: 'fp-o')
        row(c, :swap_in, 'ETH', '1', '2026-02-01T10:00:00Z', exchange: bn, group_id: 'fp', tx_id: 'fp-i', fee_currency: 'EUR', fee_amount: '10')
        row(c, :sell, 'ETH', '1', '2026-03-01T10:00:00Z', exchange: bn, quote_currency: 'EUR', quote_amount: '2000')
      } },
      # A withdrawal's fee in another coin pays for a service (Rz 6178v): 0.1 BNB, €60 for €50; then 0.9 BNB at €450.
      { name: 'at_withdrawal_fee_third_coin', country: 'AT', expect: 'diverges: D-TAX-AT-FEES', setup: lambda { |c|
        user_zone(c, 'Vienna')
        bn = venue(c, :binance)
        price('BNB', '2026-02-01', '600', 'EUR')
        price('ETH', '2026-02-01', '3000', 'EUR')
        row(c, :buy, 'BNB', '1', '2025-06-01T10:00:00Z', exchange: bn, quote_currency: 'EUR', quote_amount: '500')
        row(c, :buy, 'ETH', '1', '2025-06-02T10:00:00Z', exchange: bn, quote_currency: 'EUR', quote_amount: '2000')
        row(c, :withdrawal, 'ETH', '1', '2026-02-01T10:00:00Z', exchange: bn, fee_currency: 'BNB', fee_amount: '0.1')
        row(c, :sell, 'BNB', '0.9', '2026-03-01T10:00:00Z', exchange: bn, quote_currency: 'EUR', quote_amount: '630')
      } },
      # BTC for USDT in Kraken's grouped rows is a swap (Rz 6178b: stablecoins are crypto): the USDT carries the BTC's
      # €20,000; the USDT sold for €65,000 in 2026 → 45,000.
      { name: 'at_kraken_coin_for_stablecoin', country: 'AT', expect: 'diverges: D-TAX-COIN-QUOTE', setup: lambda { |c|
        user_zone(c, 'Vienna')
        price('BTC', '2025-06-10', '55000', 'EUR')
        price('USDT', '2025-06-10', '0.9', 'EUR')
        kraken(c, :buy, 'BTC', '1', '20000', '2024-06-01T10:00:00Z', ref: 'b1')
        kraken(c, :sell, 'BTC', '1', '60000', '2025-06-10T10:00:00Z', fiat: 'USDT', ref: 's1')
        kraken(c, :sell, 'USDT', '60000', '65000', '2026-03-01T10:00:00Z', ref: 's2')
      } },
      # USDT bought for €800, swapped for 1 ETH with a €10 fee: crypto for crypto, the fee ignored → ETH 800, gain 1,200.
      { name: 'at_stablecoin_swap_fee', country: 'AT', expect: 'diverges: D-TAX-AT-FEES', setup: lambda { |c|
        user_zone(c, 'Vienna')
        bn = venue(c, :binance)
        price('ETH', '2025-02-01', '1000', 'EUR')
        row(c, :buy, 'USDT', '1000', '2024-06-01T10:00:00Z', exchange: bn, quote_currency: 'EUR', quote_amount: '800')
        row(c, :swap_out, 'USDT', '1000', '2025-02-01T10:00:00Z', exchange: bn, group_id: 'sf', tx_id: 'sf-o')
        row(c, :swap_in, 'ETH', '1', '2025-02-01T10:00:00Z', exchange: bn, group_id: 'sf', tx_id: 'sf-i', fee_currency: 'EUR', fee_amount: '10')
        row(c, :sell, 'ETH', '1', '2026-03-01T10:00:00Z', exchange: bn, quote_currency: 'EUR', quote_amount: '2000')
      } },
      # December 2021: 1 BTC (€30,000) and €1,000 for 10 ETH: the ETH costs 31,000 (§ 6 Z 14); sold 2025 for 40,000 → 9,000.
      { name: 'at_swap_before_march_2022_top_up', country: 'AT', year: 2025, expect: 'diverges: D-TAX-AT-SWAP-1-MARCH-2022', setup: lambda { |c|
        user_zone(c, 'Vienna')
        bn = venue(c, :binance)
        price('BTC', '2021-12-01', '30000', 'EUR')
        price('ETH', '2021-12-01', '3100', 'EUR')
        row(c, :buy, 'BTC', '1', '2021-03-05T10:00:00Z', exchange: bn, quote_currency: 'EUR', quote_amount: '20000')
        row(c, :swap_out, 'BTC', '1', '2021-12-01T10:00:00Z', exchange: bn, group_id: 'tu', tx_id: 'tu-o')
        row(c, :swap_out, 'EUR', '1000', '2021-12-01T10:00:00Z', exchange: bn, group_id: 'tu', tx_id: 'tu-e')
        row(c, :swap_in, 'ETH', '10', '2021-12-01T10:00:00Z', exchange: bn, group_id: 'tu', tx_id: 'tu-i')
        row(c, :sell, 'ETH', '10', '2025-03-01T10:00:00Z', exchange: bn, quote_currency: 'EUR', quote_amount: '40000')
      } },
      { name: 'at_swap_before_march_2022_2021', country: 'AT', year: 2021, expect: 'diverges: D-TAX-AT-SWAP-1-MARCH-2022', setup: ->(c) { swap_2021.(c) } },
      { name: 'at_swap_before_march_2022_2025', country: 'AT', year: 2025, expect: 'diverges: D-TAX-AT-SWAP-1-MARCH-2022', setup: ->(c) { swap_2021.(c) } },
      # 2,000 − 1,000 − 20 = 980, old_stock false (held a year or less), listed as § 31 speculation.
      { name: 'at_sale_fee_before_march_2022', country: 'AT', year: 2022, expect: 'diverges: D-TAX-AT-SPECULATION', setup: lambda { |c|
        user_zone(c, 'Vienna')
        old_btc_2022.(c)
      } },
      # 2,000 − 1,000 = 1,000; the fee column 20.
      { name: 'at_sale_fee_not_deducted', country: 'AT', expect: 'diverges: D-TAX-AT-SALE-FEE', setup: lambda { |c|
        user_zone(c, 'Vienna')
        kraken(c, :buy, 'ETH', '1', '1000', '2025-03-01T10:00:00Z', ref: 'b1')
        kraken(c, :sell, 'ETH', '1', '2000', '2026-03-02T10:00:00Z', fee: '20', ref: 's1')
      } },
      # 0.1 BTC to Bybit, 0.0995 arriving (BTC 100,000): disposal of 0.0005 → 50.00 / 20.02 / 29.98; Bybit sells 0.0995
      # for 10,000 → cost 3,983.98, gain 6,016.02.
      { name: 'at_network_fee_disposal', country: 'AT', expect: 'diverges: D-TAX-AT-FEES', setup: lambda { |c|
        two_btc_buys.(c)
        transfer(c, 'BTC', '0.1', '2026-02-01T10:00:00Z', from: venue(c, :kraken), to: at_bybit(c), received: '0.0995')
        row(c, :sell, 'BTC', '0.0995', '2026-03-02T10:00:00Z', exchange: at_bybit(c), quote_currency: 'EUR', quote_amount: '10000')
      } },
      # An unlinked withdrawal of 0.1 BTC with its 0.0005 BTC fee on the row: the same disposal, 50.00 / 20.02 / 29.98.
      { name: 'at_unlinked_withdrawal_fee', country: 'AT', expect: 'diverges: D-TAX-AT-FEES', setup: lambda { |c|
        two_btc_buys.(c)
        row(c, :withdrawal, 'BTC', '0.1', '2026-02-01T10:00:00Z', exchange: venue(c, :kraken), fee_currency: 'BTC', fee_amount: '0.0005')
      } },
      # 0.1 BNB paid on a BTC buy (BNB 600): BNB disposal 60 / 50 / 10; the BTC sold for 55,000 → cost 50,060, gain 4,940.
      { name: 'at_bnb_fee_on_fiat_buy', country: 'AT', expect: 'diverges: D-TAX-AT-FEES', setup: lambda { |c|
        user_zone(c, 'Vienna')
        bn = venue(c, :binance)
        price('BNB', '2026-02-01', '600', 'EUR')
        row(c, :buy, 'BNB', '1', '2025-03-01T10:00:00Z', exchange: bn, quote_currency: 'EUR', quote_amount: '500')
        row(c, :buy, 'BTC', '1', '2026-02-01T10:00:00Z', exchange: bn, quote_currency: 'EUR', quote_amount: '50000', fee_currency: 'BNB', fee_amount: '0.1')
        row(c, :sell, 'BTC', '1', '2026-03-02T10:00:00Z', exchange: bn, quote_currency: 'EUR', quote_amount: '55000')
      } },
      # Four receipts in March: income and cost 4 × 20 = 80; sold for 150 → gain 70.
      { name: 'at_monthly_value_four', country: 'AT', expect: 'diverges: D-TAX-AT-MONTHLY-VALUE', setup: ->(c) { monthly.(c, %w[05 12 19 26]) } },
      # Three keep their days: income 90, cost 90, gain 60 (Rails' figures; the three lots are averaged).
      { name: 'at_monthly_value_three', country: 'AT', expect: 'diverges: D-TAX-AT-AVERAGE', setup: ->(c) { monthly.(c, %w[05 12 19]) } },
      # The § 31 sale (980) is listed as speculation; the ETH loss (−200) is not.
      { name: 'at_2022_both_regimes', country: 'AT', year: 2022, expect: 'diverges: D-TAX-AT-SPECULATION', setup: lambda { |c|
        user_zone(c, 'Vienna')
        old_btc_2022.(c)
        eth_loss_2022.(c)
      } },
      # Mining on 20 Jan 2022 (worth 300) is listed apart, not in the income section; the ETH loss −200.
      { name: 'at_income_before_regime', country: 'AT', year: 2022, expect: 'diverges: D-TAX-AT-INCOME-1-MARCH-2022', setup: lambda { |c|
        user_zone(c, 'Vienna')
        price('BTC', '2022-01-20', '30000', 'EUR')
        row(c, :mining, 'BTC', '0.01', '2022-01-20T10:00:00Z', exchange: venue(c, :kraken))
        eth_loss_2022.(c)
      } },
      # Lending income 300 and a loss of 200, both shown.
      { name: 'at_income_plus_loss', country: 'AT', expect: 'parity', setup: lambda { |c|
        user_zone(c, 'Vienna')
        price('ETH', '2026-02-01', '3000', 'EUR')
        row(c, :lending_interest, 'ETH', '0.1', '2026-02-01T10:00:00Z', exchange: venue(c, :kraken))
        kraken(c, :buy, 'SOL', '1', '500', '2026-02-05T10:00:00Z', ref: 'b1')
        kraken(c, :sell, 'SOL', '1', '300', '2026-03-02T10:00:00Z', ref: 's1')
      } },
      # 1 ETH for 2,000 and 1 ETH deposited with no price: 2 @ 2,000; 1 sold for 3,000 → cost 1,000, data_incomplete.
      { name: 'at_missing_price_in_average', country: 'AT', expect: 'diverges: D-TAX-AT-AVERAGE', setup: lambda { |c|
        user_zone(c, 'Vienna')
        kraken(c, :buy, 'ETH', '1', '2000', '2025-03-01T10:00:00Z', ref: 'b1')
        row(c, :deposit, 'ETH', '1', '2025-04-01T10:00:00Z', exchange: venue(c, :kraken))
        kraken(c, :sell, 'ETH', '1', '3000', '2026-03-02T10:00:00Z', ref: 's1')
      } },
      # +600 and −200, both § 31 speculation: they net to 400, under 440.
      { name: 'at_speculation_nets', country: 'AT', year: 2020, expect: 'diverges: D-TAX-AT-SPECULATION', setup: lambda { |c|
        user_zone(c, 'Vienna')
        kraken(c, :buy, 'BTC', '1', '1000', '2020-01-10T10:00:00Z', ref: 'b1')
        kraken(c, :buy, 'ETH', '1', '500', '2020-02-01T10:00:00Z', ref: 'b2')
        kraken(c, :sell, 'BTC', '1', '1600', '2020-06-01T10:00:00Z', ref: 's1')
        kraken(c, :sell, 'ETH', '1', '300', '2020-07-01T10:00:00Z', ref: 's2')
      } }
    ]
  end

  def at_bybit(c) = (c[:at_bybit] ||= Exchanges::Bybit.create!(name: 'Bybit', maker_fee: '0.1', taker_fee: '0.1'))

  # A12-2b: Poland's own scenarios and labels. A scenario's `nbp` ([currency, date, rate]) and `nbp_coverage`
  # ([currency, from, to]) rows are written as given: Rails has no NBP table.
  # Every shared `pl_*` scenario is Poland's cost pool (D-TAX-PL-COST-POOL), not Rails' per-sale FIFO.
  PL_EXPECT = %w[pl_buy_sell_income_deposit_stated pl_prior_year_only pl_missing_price pl_swap_chain pl_stablecoin_kept
                 pl_stablecoin_as_fiat pl_ecb_gap pl_oversold pl_lost pl_unlinked_withdrawal pl_coin_fee pl_usd_in_zloty]
              .to_h { |name| [name, 'diverges: D-TAX-PL-COST-POOL'] }.freeze
  # PIT-38 part E figures computed by hand (PLAN-A12 "A12-2b … Vectors"), in PLN; b, c, d, e, f of the report's year.
  def pl_scenarios
    k = ->(c, side, coin, qty, cash, at, fiat: 'PLN', **o) { kraken(c, side, coin, qty, cash, at, fiat:, **o) }
    carry = lambda do |c|
      k.(c, :buy, 'BTC', '1', '10000', '2024-02-05T10:00:00Z', fee: '10')
      swap(c, 'BTC', '1', 'ETH', '3', '2024-03-04T10:00:00Z', ref: 'cv1')
      k.(c, :sell, 'ETH', '1', '4000', '2024-05-06T10:00:00Z', fee: '4')
      k.(c, :sell, 'ETH', '2', '8000', '2025-04-07T10:00:00Z')
    end
    coin_fee = lambda do |c|
      FxRate.create!(currency: 'USD', date: Date.new(2026, 2, 4), rate: '1.10')
      FxRate.create!(currency: 'PLN', date: Date.new(2026, 2, 4), rate: '4.50')
      quoted(c, :buy, 'BTC', '1', '10000', '2026-02-04T10:00:00Z', fiat: 'USD', fee_currency: 'BNB', fee_amount: '0.1')
    end
    usd_0203 = { nbp: [%w[USD 2026-02-03 4.0000]], nbp_coverage: [%w[USD 2026-01-27 2026-02-03]] }
    march = lambda do |c|
      quoted(c, :buy, 'BTC', '1', '10000', '2025-03-13T10:00:00Z', fiat: 'USD', fee_currency: 'USD', fee_amount: '10')
      quoted(c, :sell, 'BTC', '0.9', '9000', '2025-03-15T10:00:00Z', fiat: 'EUR', fee_currency: 'EUR', fee_amount: '9')
      quoted(c, :sell, 'BTC', '0.1', '1000', '2025-03-17T10:00:00Z', fiat: 'EUR')
    end
    saturday = ->(c) { quoted(c, :sell, 'BTC', '0.01', '1000', '2025-03-15T10:00:00Z', fiat: 'EUR') }
    wed_fri = [%w[EUR 2025-03-12 4.1800], %w[EUR 2025-03-14 4.1790]]
    [
      # b 150,000.00; c 200,350.00; d 0; e 0; f 50,350.00; base 0; tax 0.
      pl_case('pl_cost_pool_year', year: 2025) do |c|
        k.(c, :buy, 'BTC', '1', '200000', '2025-02-03T10:00:00Z', fee: '200')
        k.(c, :sell, 'BTC', '0.5', '150000', '2025-06-02T10:00:00Z', fee: '150')
      end,
      # 2024: b 4,000, c 10,014 (the swap no cost), e 0, f 6,014.
      pl_case('pl_carry_two_years_2024', year: 2024, &carry),
      # 2025: b 8,000, c 0, d 6,014, e 1,986, f 0, base 1,986, tax 377.
      pl_case('pl_carry_two_years_2025', year: 2025, &carry),
      # 2026: all 0; the year table 2024-2026.
      pl_case('pl_carry_two_years_2026', year: 2026, &carry),
      # 2023 c 5,000 f 5,000; 2024 d 5,000 f 5,000; 2025 b 7,000, d 5,000, e 2,000, base 2,000, tax 380.
      pl_case('pl_cost_only_and_empty_years', year: 2025) do |c|
        k.(c, :buy, 'BTC', '1', '5000', '2023-03-06T10:00:00Z')
        k.(c, :sell, 'BTC', '1', '7000', '2025-03-03T10:00:00Z')
      end,
      # The BNB fee on the swap: 0.0125 × $1,000 × 4.0000 = revenue 50.00, no cost; no row for the swap.
      # b 50.00, c 10,000.00, e 0, f 9,950.00.
      pl_case('pl_swap_with_fee', **usd_0203) do |c|
        price('BNB', '2026-02-04', '1000')
        k.(c, :buy, 'BTC', '1', '10000', '2026-01-05T10:00:00Z')
        row(c, :swap_out, 'BTC', '1', '2026-02-04T10:00:00Z', exchange: venue(c, :binance), group_id: 'cv1', tx_id: 'cv1-out',
                                                                fee_currency: 'BNB', fee_amount: '0.0125')
        row(c, :swap_in, 'ETH', '30', '2026-02-04T10:00:00Z', exchange: venue(c, :binance), group_id: 'cv1', tx_id: 'cv1-in')
      end,
      # Thu buy 10,000 USD + 10 USD at Wed's 3.8496: 38,496.00 + 38.50; Sat sale 9,000 EUR, fee 9 EUR at Fri's 4.1790:
      # 37,611.00 and 37.61; Mon sale 1,000 EUR at Fri's 4.1790 (not Mon's 4.1777): 4,179.00.
      # b 41,790.00; c 38,572.11; e 3,217.89; base 3,218; tax 611.
      pl_case('pl_nbp_day_before', year: 2025, nbp: [%w[USD 2025-03-12 3.8496], %w[EUR 2025-03-14 4.1790], %w[EUR 2025-03-17 4.1777]],
                                   nbp_coverage: [%w[USD 2025-03-05 2025-03-17], %w[EUR 2025-03-05 2025-03-17]], &march),
      # 1,000 USD at 3.8496 = 3,849.60; the 1 EUR fee at 4.1800 = 4.18. c 3,853.78, f 3,853.78.
      pl_case('pl_usd_trade_eur_fee', year: 2025, nbp: [%w[USD 2025-03-12 3.8496], %w[EUR 2025-03-12 4.1800]],
                                      nbp_coverage: [%w[USD 2025-03-05 2025-03-12], %w[EUR 2025-03-05 2025-03-12]]) do |c|
        quoted(c, :buy, 'BTC', '0.1', '1000', '2025-03-13T10:00:00Z', fiat: 'USD', fee_currency: 'EUR', fee_amount: '1')
      end,
      # Two cost rows of 10,000.00: c 20,000.00, f 20,000.00.
      pl_case('pl_grouped_vs_single_row') do |c|
        k.(c, :buy, 'BTC', '0.1', '10000', '2026-02-02T10:00:00Z')
        quoted(c, :buy, 'BTC', '0.1', '10000', '2026-02-02T11:00:00Z')
      end,
      # b 10,000; c 8,025; e 1,975; base 1,975; tax 375.
      pl_case('pl_sale_fee_is_cost') do |c|
        k.(c, :buy, 'ETH', '1', '8000', '2026-01-05T10:00:00Z')
        k.(c, :sell, 'ETH', '1', '10000', '2026-03-02T10:00:00Z', fee: '25')
      end,
      # ECB's cross (4.0909) is not read. Cost 40,000.00 (BTC) + 200.00 (0.1 BNB × $500 × 4.0000); revenue 200.00 (the
      # BNB settled). b 200.00, c 40,200.00, e 0, f 40,000.00.
      pl_case('pl_coin_fee_on_fiat_trade', **usd_0203) { |c| price('BNB', '2026-02-04', '500'); coin_fee.(c) },
      # No BNB price: the banner, both fee components 0.00 and listed. b 0, c 40,000.00, f 40,000.00.
      pl_case('pl_coin_fee_on_fiat_trade_unpriced', **usd_0203, &coin_fee),
      # The staking reward is listed, no figure. b 1,500; c 0; e 1,500; base 1,500; tax 285.
      pl_case('pl_rewards_not_revenue') do |c|
        row(c, :staking_reward, 'ETH', '0.1', '2026-02-01T00:00:00Z', exchange: venue(c, :kraken))
        k.(c, :sell, 'ETH', '0.1', '1500', '2026-03-02T10:00:00Z')
      end,
      # Before 2019: 4,000.00 (its own line). b 1,000; c 0; d 0; e 1,000; base 1,000; tax 190.
      pl_case('pl_pre_2019_costs', year: 2019) do |c|
        k.(c, :buy, 'BTC', '1', '4000', '2018-06-04T10:00:00Z')
        k.(c, :sell, 'BTC', '0.25', '1000', '2019-06-03T10:00:00Z')
      end,
      # No NBP EUR rate: the banner, revenue 0.00, listed. b 0.
      pl_case('pl_missing_nbp_rate') { |c| kraken(c, :sell, 'BTC', '0.01', '1000', '2026-03-04T10:00:00Z') },
      # b 102.50; c 100.00; e 2.50; base 3; tax 1 (0.57).
      pl_case('pl_rounding') do |c|
        k.(c, :buy, 'BTC', '0.001', '100', '2026-01-05T10:00:00Z')
        k.(c, :sell, 'BTC', '0.001', '102.50', '2026-03-02T10:00:00Z')
      end,
      # USDT is a virtual currency whatever the flag: the swap is no revenue. b 0; c 10,000; f 10,000.
      pl_case('pl_stablecoin_flag_ignored', stablecoin: true) do |c|
        k.(c, :buy, 'BTC', '1', '10000', '2026-01-05T10:00:00Z')
        swap(c, 'BTC', '0.5', 'USDT', '16000', '2026-02-04T10:00:00Z', ref: 'cv1')
      end,
      # D-TAX-STABLECOIN-QUESTION: under the flag a single row quoted in USDT is split as without it (the flag had kept it
      # whole for PL). The swap is no revenue and no cost; its 30 USDT fee is revenue at its value (D-TAX-PL-SWAP-FEE):
      # 30 × $1 × 4.0000 = 120.00. b 120.00; c 10,000.00; f 9,880.00.
      pl_case('pl_stablecoin_quoted_flag_on', stablecoin: true, expect: 'diverges: D-TAX-STABLECOIN-QUESTION', **usd_0203) do |c|
        price('USDT', '2026-02-04', '1')
        k.(c, :buy, 'BTC', '1', '10000', '2026-01-05T10:00:00Z')
        quoted(c, :sell, 'BTC', '0.5', '30000', '2026-02-04T10:00:00Z', fiat: 'USDT', fee_currency: 'USDT', fee_amount: '30')
      end,
      # D-TAX-PL-DECLARED-CARRY: column d as the user declares it for a year (`pl_carry` rows: [user, year, costs], the
      # user "self" or "other"; Rails has no such input and ignores them). History from 2026 only, 6,014 carried:
      # b 8,000; c 0; d 6,014; e 1,986; base 1,986; tax 377 (rebuilt: d 0, tax 1,520). Another user's row is not read.
      pl_case('pl_declared_carry_2026', expect: 'diverges: D-TAX-PL-DECLARED-CARRY',
              pl_carry: [['self', 2026, '6014'], ['other', 2026, '100']]) do |c|
        k.(c, :sell, 'BTC', '1', '8000', '2026-03-02T10:00:00Z')
      end,
      # Rebuilt 2024: f 6,014; declared 2025 = 5,000: d 5,000; e 3,000; base 3,000; tax 570.
      pl_case('pl_declared_differs_2025', year: 2025, expect: 'diverges: D-TAX-PL-DECLARED-CARRY',
              pl_carry: [['self', 2025, '5000']], &carry),
      # Declared 2025 = 0 overrides the rebuilt 6,014: d 0; e 8,000; tax 1,520.
      pl_case('pl_declared_zero_2025', year: 2025, expect: 'diverges: D-TAX-PL-DECLARED-CARRY',
              pl_carry: [['self', 2025, '0']], &carry),
      # Declared 2024 = 1,000: 2024 b 4,000, c 10,014, d 1,000, f 7,014; declared 2025 = 500 resets the chain:
      # d 500; e 7,500; base 7,500; tax 1,425.
      pl_case('pl_declared_twice_2025', year: 2025, expect: 'diverges: D-TAX-PL-DECLARED-CARRY',
              pl_carry: [['self', 2024, '1000.00'], ['self', 2025, '500']], &carry),
      # A declaration for a later year is not read: the file of pl_carry_two_years_2024.
      pl_case('pl_declared_future_ignored_2024', year: 2024, pl_carry: [['self', 2025, '9999']], &carry),
      # Declared 2024 = 3,000 and nothing synced before 2026: 2024 b 0, c 0, d 3,000, f 3,000 (declared); 2025 d 3,000,
      # f 3,000; 2026 sell 1,000: d 3,000; e 0; f 2,000; tax 0 (carried from 2024's: no d_declared line).
      pl_case('pl_declared_across_empty_years', expect: 'diverges: D-TAX-PL-DECLARED-CARRY',
              pl_carry: [['self', 2024, '3000']]) do |c|
        k.(c, :sell, 'BTC', '0.1', '1000', '2026-03-02T10:00:00Z')
      end,
      # No synced row at all; declared 2025 = 1,000: b 0; c 0; d 1,000; e 0; f 1,000.
      pl_case('pl_declared_only_2025', year: 2025, expect: 'diverges: D-TAX-PL-DECLARED-CARRY', pl_carry: [['self', 2025, '1000']]) { |_c| nil },
      # Before 2019: 4,000 synced (its line); declared 2019 = 3,000 (1,000 deducted before): b 1,000; c 0; d 3,000; e 0;
      # f 2,000; tax 0.
      pl_case('pl_declared_before_2019', year: 2019, expect: 'diverges: D-TAX-PL-DECLARED-CARRY',
              pl_carry: [['self', 2019, '3000']]) do |c|
        k.(c, :buy, 'BTC', '1', '4000', '2018-06-04T10:00:00Z')
        k.(c, :sell, 'BTC', '0.25', '1000', '2019-06-03T10:00:00Z')
      end,
      # Listed with the PL hint, no figure. c 10,000; f 10,000.
      pl_case('pl_unlinked_withdrawal_listed') do |c|
        k.(c, :buy, 'BTC', '1', '10000', '2026-01-05T10:00:00Z')
        row(c, :withdrawal, 'BTC', '0.4', '2026-02-04T10:00:00Z', exchange: venue(c, :kraken))
      end,
      # Not served: the regime starts in 2019.
      pl_case('pl_before_2019_refused', year: 2018, expect: 'not ported (D-TAX-NOT-PORTED-GATE)') do |c|
        k.(c, :buy, 'BTC', '1', '4000', '2018-06-04T10:00:00Z')
        k.(c, :sell, 'BTC', '1', '5000', '2018-09-03T10:00:00Z')
      end,
      # Saturday's sale, Friday's table stored but coverage ends Wednesday: missing, 0.00 (Wednesday's 4.1800 unused).
      pl_case('pl_nbp_coverage_gap', year: 2025, nbp: wed_fri, nbp_coverage: [%w[EUR 2025-03-05 2025-03-12]], &saturday),
      # The same with Friday covered: 1,000 × 4.1790 = 4,179.00. b 4,179.00, e 4,179.00, base 4,179, tax 794.
      pl_case('pl_nbp_coverage_full', year: 2025, nbp: wed_fri, nbp_coverage: [%w[EUR 2025-03-05 2025-03-14]], &saturday)
    ]
  end

  # A12-3: Denmark's own scenarios and labels (PLAN-A12 "A12-3 … Vectors", figures by hand). Every DK scenario runs on
  # the ECB's DKK rate 7.46 on every booked day and each EUR price × 7.46 as a DKK price (`dk_fx`): DKK = EUR × 7.46.
  DK_EXPECT = {
    'dk_buy_sell_income_deposit_stated' => 'diverges: D-TAX-DK-PER-PART',
    # Rails' header carries its loss_denied column even with no row.
    'dk_prior_year_only' => 'diverges: D-TAX-DK-NO-WASH-SALE',
    'dk_missing_price' => 'diverges: D-TAX-DK-DEPOSIT-ZERO',
    'dk_swap_chain' => 'diverges: D-TAX-DK-SWAP-VALUE',
    'dk_stablecoin_kept' => 'diverges: D-TAX-DK-STABLECOIN',
    'dk_stablecoin_as_fiat' => 'diverges: D-TAX-DK-STABLECOIN',
    'dk_ecb_gap' => 'diverges: D-TAX-DK-RUBRIK',
    'dk_oversold' => 'diverges: D-TAX-DK-PER-PART',
    'dk_lost' => 'diverges: D-TAX-DK-RUBRIK',
    'dk_unlinked_withdrawal' => 'diverges: D-TAX-DK-RUBRIK',
    'dk_coin_fee' => 'diverges: D-TAX-DK-PER-PART'
  }.freeze

  def dk_fx(gaps)
    days = AccountTransaction.pluck(:transacted_at).map { |t| t.utc.to_date }.uniq - gaps.map { |d| Date.parse(d) }
    FxRate.insert_all(days.map { |d| { currency: 'DKK', date: d, rate: '7.46' } }, unique_by: %i[currency date]) if days.any?
    HistoricalPrice.where(currency: 'EUR').to_a.each do |p|
      next if HistoricalPrice.exists?(asset: p.asset, currency: 'DKK', date: p.date)

      HistoricalPrice.create!(asset: p.asset, currency: 'DKK', date: p.date, price: p.price * BigDecimal('7.46'))
    end
  end

  def dk_case(name, expect, year: 2026, &setup)
    { name:, country: 'DK', year:, expect: "diverges: #{expect}", setup: lambda { |c|
      user_zone(c, 'Copenhagen')
      setup.(c)
    } }
  end

  def dk_scenarios
    k = ->(c, side, coin, qty, cash, at, **o) { kraken(c, side, coin, qty, cash, at, fiat: 'DKK', **o) }
    # BTC bought for DKK on Kraken in 2025 and moved to Binance, where it is swapped in 2026.
    btc_to_binance = lambda do |c|
      price('BTC', '2025-07-01', '350000', 'DKK')
      k.(c, :buy, 'BTC', '1', '300000', '2025-06-01T10:00:00Z')
      transfer(c, 'BTC', '1', '2025-07-01T10:00:00Z', from: venue(c, :kraken), to: venue(c, :binance))
    end
    usd = ->(c, side, coin, qty, cash, at) { quoted(c, side, coin, qty, cash, at, fiat: 'USD') }
    [
      # skat.dk's Markus: +300; then +100 (0.5 of January's) and −300 (February's), not netted → rubrik 20 400, 58 300.
      dk_case('dk_markus', 'D-TAX-DK-PER-PART') do |c|
        k.(c, :buy, 'BTC', '1', '1000', '2026-01-10T10:00:00Z')
        k.(c, :buy, 'BTC', '1', '1500', '2026-02-10T10:00:00Z')
        k.(c, :sell, 'BTC', '0.5', '800', '2026-03-10T10:00:00Z')
        k.(c, :sell, 'BTC', '1.5', '1800', '2026-04-10T10:00:00Z')
      end,
      # −5,000 in rubrik 58; no loss denied, no loss_denied column.
      dk_case('dk_no_wash_sale', 'D-TAX-DK-NO-WASH-SALE') do |c|
        k.(c, :buy, 'ETH', '1', '20000', '2026-01-10T10:00:00Z')
        k.(c, :buy, 'ETH', '1', '25000', '2026-03-10T10:00:00Z')
        k.(c, :sell, 'ETH', '1', '15000', '2026-04-10T10:00:00Z')
      end,
      # 1 BTC (700,000) for 20 ETH (680,000): one value, the BTC side's → BTC 700,000 / 300,000 / 400,000; ETH cost
      # 700,000, sold for 690,000 → −10,000.
      dk_case('dk_swap_one_value', 'D-TAX-DK-SWAP-VALUE') do |c|
        btc_to_binance.(c)
        price('BTC', '2026-02-01', '700000', 'DKK')
        price('ETH', '2026-02-01', '34000', 'DKK')
        price('ETH', '2026-02-02', '34000', 'DKK')
        swap(c, 'BTC', '1', 'ETH', '20', '2026-02-01T10:00:00Z', ref: 'sv1')
        transfer(c, 'ETH', '20', '2026-02-02T10:00:00Z', from: venue(c, :binance), to: venue(c, :kraken))
        k.(c, :sell, 'ETH', '20', '690000', '2026-03-01T10:00:00Z')
      end,
      # 0.1 BTC for 10,850 USDT (10,850 × 7.46 / 1.17 = 69,180.34) → BTC 69,180.34 / 30,000 / 39,180.34 in rubrik 20;
      # the USDT sold for 70,000 DKK → 819.66, apart.
      dk_case('dk_stablecoin_apart', 'D-TAX-DK-STABLECOIN') do |c|
        btc_to_binance.(c)
        price('BTC', '2026-02-01', '700000', 'DKK')
        swap(c, 'BTC', '0.1', 'USDT', '10850', '2026-02-01T10:00:00Z', ref: 'sa1')
        transfer(c, 'USDT', '10850', '2026-02-01T12:00:00Z', from: venue(c, :binance), to: venue(c, :kraken))
        k.(c, :sell, 'USDT', '10850', '70000', '2026-02-02T10:00:00Z')
      end,
      # An unlinked deposit of 1 ETH (worth 20,000) sold for 25,000 → cost 0, gain 25,000.
      dk_case('dk_deposit_zero', 'D-TAX-DK-DEPOSIT-ZERO') do |c|
        price('ETH', '2026-01-10', '20000', 'DKK')
        row(c, :deposit, 'ETH', '1', '2026-01-10T10:00:00Z', exchange: venue(c, :kraken))
        k.(c, :sell, 'ETH', '1', '25000', '2026-03-01T10:00:00Z')
      end,
      # Staking 0.1 ETH (2,000) and lending 1 SOL (1,000), no sale → rubrik 20 3,000, the summary shown.
      dk_case('dk_income_only', 'D-TAX-DK-RUBRIK') do |c|
        price('ETH', '2026-02-01', '20000', 'DKK')
        price('SOL', '2026-03-01', '1000', 'DKK')
        row(c, :staking_reward, 'ETH', '0.1', '2026-02-01T10:00:00Z', exchange: venue(c, :kraken))
        row(c, :lending_interest, 'SOL', '1', '2026-03-01T10:00:00Z', exchange: venue(c, :kraken))
      end,
      # Mining 0.1 BTC worth 20,000, sold for 15,000 → row −5,000 shown, rubrik 58 0; receipt and sale listed.
      dk_case('dk_mining_sold_at_loss', 'D-TAX-DK-RUBRIK') do |c|
        price('BTC', '2026-02-01', '200000', 'DKK')
        row(c, :mining, 'BTC', '0.1', '2026-02-01T10:00:00Z', exchange: venue(c, :kraken))
        k.(c, :sell, 'BTC', '0.1', '15000', '2026-03-01T10:00:00Z')
      end,
      # DAI bought and held: the stablecoin NOTE lists DAI as not classified.
      dk_case('dk_dai_only', 'D-TAX-DK-STABLECOIN') do |c|
        usd.(c, :buy, 'DAI', '1000', '1000', '2026-02-01T10:00:00Z')
      end,
      # USDT bought in 2025 and untouched in 2026: held at the year's start, so the NOTE shows USDT.
      dk_case('dk_usdt_held_no_activity', 'D-TAX-DK-STABLECOIN') do |c|
        usd.(c, :buy, 'USDT', '1000', '1000', '2025-06-01T10:00:00Z')
      end,
      # USDT bought and sold out in 2026: its realised result in the NOTE, out of the totals.
      dk_case('dk_usdt_sold_out', 'D-TAX-DK-STABLECOIN') do |c|
        usd.(c, :buy, 'USDT', '1000', '1000', '2026-02-01T10:00:00Z')
        usd.(c, :sell, 'USDT', '1000', '1010', '2026-03-01T10:00:00Z')
      end,
      # DAI 1,000 bought for 1,000 USD, sold for 1,100 USD → 7,013.68 / 6,376.07 / 637.61 in rubrik 20; DAI not classified.
      dk_case('dk_dai_gain', 'D-TAX-DK-STABLECOIN') do |c|
        usd.(c, :buy, 'DAI', '1000', '1000', '2026-02-01T10:00:00Z')
        usd.(c, :sell, 'DAI', '1000', '1100', '2026-03-01T10:00:00Z')
      end
    ]
  end

  # A12-3: Greece. No tax on an individual's crypto gains is in force up to 2026: rows as Rails, Σ gains and Σ losses,
  # no taxable income or tax line, NOTE gr_no_tax. Every shared GR scenario with a disposal shows it.
  GR_EXPECT = %w[gr_buy_sell_income_deposit_stated gr_missing_price gr_swap_chain gr_stablecoin_kept gr_stablecoin_as_fiat
                 gr_ecb_gap gr_oversold gr_lost gr_unlinked_withdrawal gr_coin_fee]
              .to_h { |name| [name, 'diverges: D-TAX-GR-NO-TAX'] }.freeze
  def gr_scenarios
    two_trades = lambda do |c|
      user_zone(c, 'Athens')
      kraken(c, :buy, 'BTC', '1', '10000', '2025-01-10T10:00:00Z')
      kraken(c, :buy, 'ETH', '1', '20000', '2025-02-10T10:00:00Z')
      kraken(c, :sell, 'BTC', '1', '30000', '2025-03-10T10:00:00Z')
      kraken(c, :sell, 'ETH', '1', '9000', '2025-04-10T10:00:00Z')
    end
    [
      # +20,000 and −11,000 → gains 20,000.00, losses 11,000.00, no tax line, NOTE.
      { name: 'gr_gains_and_losses', country: 'GR', year: 2025, expect: 'diverges: D-TAX-GR-NO-TAX', setup: two_trades },
      # 2027 is refused as not ported until enacted law is reviewed.
      { name: 'gr_2027_not_ported', country: 'GR', year: 2027, expect: 'parity', setup: two_trades }
    ]
  end

  # A12-4: Italy by its own law (PLAN-A12 "A12-4", r0-r5; every figure computed by hand there). LIFO across every wallet,
  # crypto-for-crypto no realisation (but an exchange with an e-money token, USDC or EURC), fees counted nowhere, the
  # quadro RT summary; 26% to 2025, 33% from 2026 (EURC 26%). Rome. Every IT scenario diverges (the columns change).
  IT_EXPECT = {
    'it_buy_sell_income_deposit_stated' => 'diverges: D-TAX-IT-RT', 'it_prior_year_only' => 'diverges: D-TAX-IT-THRESHOLD',
    'it_missing_price' => 'diverges: D-TAX-IT-DEPOSIT-ZERO', 'it_swap_chain' => 'diverges: D-TAX-IT-SWAPS',
    'it_stablecoin_kept' => 'diverges: D-TAX-IT-SWAPS', 'it_stablecoin_as_fiat' => 'diverges: D-TAX-IT-SWAPS',
    'it_ecb_gap' => 'diverges: D-TAX-IT-RT', 'it_oversold' => 'diverges: D-TAX-UNCOVERED', 'it_lost' => 'diverges: D-TAX-LOST',
    'it_unlinked_withdrawal' => 'diverges: D-TAX-UNLINKED-DISCLOSED', 'it_coin_fee' => 'diverges: D-TAX-IT-FEES'
  }.freeze
  def it_eurc(_c) = Asset.find_by(symbol: 'EURC') || Asset.create!(external_id: 'euro-coin', symbol: 'EURC', name: 'EURC', category: 'Cryptocurrency')
  def it_scenarios
    it = ->(name, expect, year: 2026, &setup) { { name:, country: 'IT', year:, expect:, setup: ->(c) { user_zone(c, 'Rome'); setup.(c) } } }
    k = ->(c) { venue(c, :kraken) }
    b = ->(c) { venue(c, :binance) }
    # 1 BTC bought for 20,000 on Kraken (02-01) and moved to Binance (02-05).
    btc_on_binance = lambda do |c|
      price('BTC', '2026-02-05', '21000', 'EUR')
      kraken(c, :buy, 'BTC', '1', '20000', '2026-02-01T10:00:00Z')
      transfer(c, 'BTC', '1', '2026-02-05T10:00:00Z', from: k.(c), to: b.(c))
    end
    # ... swapped for 30,000 USDC (06-01): 25,641.03 / 20,000 / 5,641.03.
    btc_to_usdc = lambda do |c|
      btc_on_binance.(c)
      price('BTC', '2026-06-01', '30000', 'EUR')
      swap(c, 'BTC', '1', 'USDC', '30000', '2026-06-01T10:00:00Z', ref: 'cv1')
    end
    # ... swapped for 25,000 EURC (03-01) at 33%; 10,000 EURC for 4 ETH (04-01) at 26%; 15,000 EURC to Kraken.
    eurc = lambda do |c|
      it_eurc(c)
      btc_on_binance.(c)
      %w[2026-03-01 2026-04-01 2026-04-10 2026-05-01].each { |d| price('EURC', d, '1', 'EUR') }
      price('BTC', '2026-03-01', '25000', 'EUR')
      price('ETH', '2026-04-01', '2525', 'EUR')
      swap(c, 'BTC', '1', 'EURC', '25000', '2026-03-01T10:00:00Z', ref: 'cv1')
      swap(c, 'EURC', '10000', 'ETH', '4', '2026-04-01T10:00:00Z', ref: 'cv2')
      transfer(c, 'EURC', '15000', '2026-04-10T10:00:00Z', from: b.(c), to: k.(c))
    end
    two_years = lambda do |c|
      kraken(c, :buy, 'ETH', '1', '1000', '2025-02-03T10:00:00Z')
      kraken(c, :sell, 'ETH', '1', '2000', '2025-06-02T10:00:00Z')
      kraken(c, :buy, 'BTC', '1', '1000', '2026-02-02T10:00:00Z')
      kraken(c, :sell, 'BTC', '1', '2000', '2026-06-01T10:00:00Z')
    end
    one_trade = ->(year, bought, sold) { ->(c) { kraken(c, :buy, 'ETH', '1', bought, "#{year}-02-01T10:00:00Z"); kraken(c, :sell, 'ETH', '1', sold, "#{year}-06-01T10:00:00Z") } }
    [
      # 2024: LIFO takes the 30,000 lot: 25,000 / 30,000 / −5,000; threshold 2,000.00, taxable 0.00, loss_carried 3,000.00, tax 0.00.
      it.('it_lifo_loss_2024', 'diverges: D-TAX-IT-RT', year: 2024) do |c|
        kraken(c, :buy, 'BTC', '1', '10000', '2024-01-10T10:00:00Z')
        kraken(c, :buy, 'BTC', '1', '30000', '2024-03-10T10:00:00Z')
        kraken(c, :sell, 'BTC', '1', '25000', '2024-06-10T10:00:00Z')
      end,
      # LIFO across wallets: the Binance lot (30,000) goes with the Kraken sale: −5,000; loss_carried 5,000.00.
      it.('it_lifo_pooled_venues', 'diverges: D-TAX-IT-RT') do |c|
        kraken(c, :buy, 'BTC', '1', '10000', '2026-01-10T10:00:00Z')
        row(c, :buy, 'BTC', '1', '2026-02-10T10:00:00Z', exchange: b.(c), quote_currency: 'EUR', quote_amount: '30000')
        kraken(c, :sell, 'BTC', '1', '25000', '2026-03-10T10:00:00Z')
      end,
      # The swap carries the BTC's 20,000: 33,000 / 20,000 / 13,000, tax_33 4,290.00 (Rails: 10,000 + 3,000 in two rows).
      it.('it_swap_not_taxable', 'diverges: D-TAX-IT-SWAPS') do |c|
        btc_on_binance.(c)
        price('BTC', '2026-06-01', '30000', 'EUR')
        price('ETH', '2026-06-01', '2000', 'EUR')
        price('ETH', '2026-06-05', '2100', 'EUR')
        swap(c, 'BTC', '1', 'ETH', '15', '2026-06-01T10:00:00Z', ref: 'cv1')
        transfer(c, 'ETH', '15', '2026-06-05T10:00:00Z', from: b.(c), to: k.(c))
        kraken(c, :sell, 'ETH', '15', '33000', '2026-09-01T10:00:00Z')
      end,
      # BTC → USDC is a disposal at the USDC received: 25,641.03 / 20,000 / 5,641.03; USDC sold for 25,700 EUR: 58.97 →
      # result 5,700.00, tax_33 1,881.00.
      it.('it_swap_emt_taxable', 'diverges: D-TAX-IT-SWAPS') do |c|
        btc_to_usdc.(c)
        transfer(c, 'USDC', '30000', '2026-06-03T10:00:00Z', from: b.(c), to: k.(c))
        kraken(c, :sell, 'USDC', '30000', '25700', '2026-06-05T10:00:00Z')
      end,
      # 33% on the BTC (5,000), 26% on the EURC exchanged for ETH (100); 15,000 EURC sold for 15,000 EUR: no row, listed
      # (0.00) → tax_33 1,650.00, tax_26_emt 26.00.
      it.('it_eurc_26', 'diverges: D-TAX-IT-EMT-RATE') do |c|
        eurc.(c)
        kraken(c, :sell, 'EURC', '15000', '15000', '2026-05-01T10:00:00Z')
      end,
      # The same sold for 15,060 EUR: listed with 60.00, not taxed.
      it.('it_eurc_premium', 'diverges: D-TAX-IT-EMT-RATE') do |c|
        eurc.(c)
        kraken(c, :sell, 'EURC', '15000', '15060', '2026-05-01T10:00:00Z')
      end,
      # USDC (cost 25,641.03) for 25,600 EURC: 25,600 / 25,641.03 / −41.03 at 33%; with the BTC row (5,641.03) → result
      # 5,600.00, tax_33 1,848.00.
      it.('it_emt_to_emt', 'diverges: D-TAX-IT-SWAPS') do |c|
        it_eurc(c)
        btc_to_usdc.(c)
        price('EURC', '2026-06-10', '1', 'EUR')
        swap(c, 'USDC', '30000', 'EURC', '25600', '2026-06-10T10:00:00Z', ref: 'cv2')
      end,
      # EURC staking 100: the 26% bucket → staking 100.00, result 100.00, tax_33 0.00, tax_26_emt 26.00.
      it.('it_eurc_staking', 'diverges: D-TAX-IT-EMT-RATE') do |c|
        it_eurc(c)
        price('EURC', '2026-03-01', '1', 'EUR')
        row(c, :staking_reward, 'EURC', '100', '2026-03-01T00:00:00Z', exchange: k.(c))
      end,
      # One group: 1 BTC → 15,000 USDC (12,820.51) + 5 ETH (10,000): BTC 22,820.51 / 20,000 / 2,820.51 → tax_33 930.77.
      it.('it_emt_mixed_group', 'diverges: D-TAX-IT-SWAPS') do |c|
        btc_on_binance.(c)
        price('BTC', '2026-06-01', '23000', 'EUR')
        price('ETH', '2026-06-01', '2000', 'EUR')
        row(c, :swap_out, 'BTC', '1', '2026-06-01T10:00:00Z', exchange: b.(c), group_id: 'cv1', tx_id: 'cv1-out-BTC')
        row(c, :swap_in, 'USDC', '15000', '2026-06-01T10:00:00Z', exchange: b.(c), group_id: 'cv1', tx_id: 'cv1-in-USDC')
        row(c, :swap_in, 'ETH', '5', '2026-06-01T10:00:00Z', exchange: b.(c), group_id: 'cv1', tx_id: 'cv1-in-ETH')
      end,
      # A 300 USDC fee on the in-leg: BTC proceeds 25,641.03 (before the fee); USDC 29,700 at 25,384.62, sold for 25,500
      # EUR: 115.38 → result 5,756.41, tax_33 1,899.62.
      it.('it_emt_fee', 'diverges: D-TAX-IT-FEES') do |c|
        btc_on_binance.(c)
        price('BTC', '2026-06-01', '30000', 'EUR')
        row(c, :swap_out, 'BTC', '1', '2026-06-01T10:00:00Z', exchange: b.(c), group_id: 'cv1', tx_id: 'cv1-out-BTC')
        row(c, :swap_in, 'USDC', '30000', '2026-06-01T10:00:00Z', exchange: b.(c), group_id: 'cv1', tx_id: 'cv1-in-USDC', fee_currency: 'USDC', fee_amount: '300')
        transfer(c, 'USDC', '29700', '2026-06-03T10:00:00Z', from: b.(c), to: k.(c))
        kraken(c, :sell, 'USDC', '29700', '25500', '2026-06-05T10:00:00Z')
      end,
      # Fees count nowhere: 15,000 / 10,000 / 5,000, fee 150 shown.
      it.('it_fees_ignored', 'diverges: D-TAX-IT-FEES') do |c|
        kraken(c, :buy, 'BTC', '1', '10000', '2026-01-10T10:00:00Z', fee: '100')
        kraken(c, :sell, 'BTC', '1', '15000', '2026-03-10T10:00:00Z', fee: '150')
      end,
      # An own-coin fee's units leave at their share: 0.99 BTC at 9,900; sold for 14,850 → 4,950.
      it.('it_fee_own_coin', 'diverges: D-TAX-IT-FEES') do |c|
        price('BTC', '2026-01-10', '10000', 'EUR')
        quoted(c, :buy, 'BTC', '1', '10000', '2026-01-10T10:00:00Z', fiat: 'EUR', fee_currency: 'BTC', fee_amount: '0.01')
        quoted(c, :sell, 'BTC', '0.99', '14850', '2026-03-10T10:00:00Z', fiat: 'EUR')
      end,
      # An unlinked deposit costs 0: 2,500 / 0 / 2,500.
      it.('it_deposit_zero', 'diverges: D-TAX-IT-DEPOSIT-ZERO') do |c|
        price('ETH', '2026-02-02', '2000', 'EUR')
        row(c, :deposit, 'ETH', '1', '2026-02-02T10:00:00Z', exchange: k.(c))
        kraken(c, :sell, 'ETH', '1', '2500', '2026-03-02T10:00:00Z')
      end,
      # Staking only: staking 2,000.00, result 2,000.00, tax_33 660.00.
      it.('it_staking_only', 'diverges: D-TAX-IT-INCOME') do |c|
        price('ETH', '2026-03-01', '2000', 'EUR')
        row(c, :staking_reward, 'ETH', '1', '2026-03-01T00:00:00Z', exchange: k.(c))
      end,
      # The provento is the row's gross value (2,000.00); the lot 0.9 at 1,800, sold for 1,900 → +100 → result 2,100.00,
      # tax_33 693.00.
      it.('it_staking_withheld', 'diverges: D-TAX-IT-INCOME') do |c|
        price('ETH', '2026-03-01', '2000', 'EUR')
        row(c, :staking_reward, 'ETH', '1', '2026-03-01T00:00:00Z', exchange: k.(c), fee_currency: 'ETH', fee_amount: '0.1')
        kraken(c, :sell, 'ETH', '0.9', '1900', '2026-06-01T10:00:00Z')
      end,
      # 2023: result 2,100 → taxable 100.00, tax_26 26.00; cliff_2023 546.00.
      it.('it_threshold_2023', 'diverges: D-TAX-IT-THRESHOLD', year: 2023, &one_trade.('2023', '1000', '3100')),
      # 2024: result 1,900 → taxable 0.00, tax 0.00.
      it.('it_threshold_2024_below', 'diverges: D-TAX-IT-THRESHOLD', year: 2024, &one_trade.('2024', '1000', '2900')),
      # 2024: staking 500 + gain 1,700 → result 2,200, taxable 200.00, tax 52.00.
      it.('it_threshold_2024_staking', 'diverges: D-TAX-IT-THRESHOLD', year: 2024) do |c|
        price('ETH', '2024-03-01', '2000', 'EUR')
        row(c, :staking_reward, 'ETH', '0.25', '2024-03-01T00:00:00Z', exchange: k.(c))
        kraken(c, :buy, 'BTC', '1', '10000', '2024-02-01T10:00:00Z')
        kraken(c, :sell, 'BTC', '1', '11700', '2024-06-01T10:00:00Z')
      end,
      # −2,500: 2023 carries 500.00 (above the 2,000), 2025 all 2,500.00.
      it.('it_loss_2023', 'diverges: D-TAX-IT-THRESHOLD', year: 2023, &one_trade.('2023', '5000', '2500')),
      it.('it_loss_2025', 'diverges: D-TAX-IT-RT', year: 2025, &one_trade.('2025', '5000', '2500')),
      # Gain 1,000 in 2025 → 260.00; gain 1,000 in 2026 → 330.00.
      it.('it_rate_2025', 'diverges: D-TAX-IT-RT', year: 2025, &two_years),
      it.('it_rate_2026', 'diverges: D-TAX-IT-RT', &two_years),
      # Under LIFO the swap's ETH is the newest (dated at the swap, the BTC's 20,000 carried): 1 ETH sold for 3,500 costs 2,000.
      it.('it_swap_into_held_coin', 'diverges: D-TAX-IT-SWAPS') do |c|
        price('BTC', '2024-02-01', '40000', 'EUR')
        price('BTC', '2026-03-01', '60000', 'EUR')
        price('ETH', '2026-03-01', '3000', 'EUR')
        price('ETH', '2026-03-05', '3100', 'EUR')
        kraken(c, :buy, 'BTC', '1', '20000', '2024-01-10T10:00:00Z')
        transfer(c, 'BTC', '1', '2024-02-01T10:00:00Z', from: k.(c), to: b.(c))
        kraken(c, :buy, 'ETH', '1', '3000', '2025-01-10T10:00:00Z')
        swap(c, 'BTC', '1', 'ETH', '10', '2026-03-01T10:00:00Z', ref: 'cv1')
        transfer(c, 'ETH', '10', '2026-03-05T10:00:00Z', from: b.(c), to: k.(c))
        kraken(c, :sell, 'ETH', '1', '3500', '2026-06-01T10:00:00Z')
      end,
      # A swap-in with no out-leg has no documented cost: 0 (listed as unmatched): 2,500 / 0 / 2,500.
      it.('it_orphan_swap_in', 'diverges: D-TAX-IT-DEPOSIT-ZERO') do |c|
        price('ETH', '2026-02-02', '2000', 'EUR')
        price('ETH', '2026-02-05', '2100', 'EUR')
        row(c, :swap_in, 'ETH', '1', '2026-02-02T10:00:00Z', exchange: b.(c), group_id: 'cv1', tx_id: 'cv1-in-ETH')
        transfer(c, 'ETH', '1', '2026-02-05T10:00:00Z', from: b.(c), to: k.(c))
        kraken(c, :sell, 'ETH', '1', '2500', '2026-03-02T10:00:00Z')
      end,
      # A 0.1 ETH fee on a non-taxable swap-in: 9.9 ETH at 19,800 (200 consumed); sold for 30,000 → 10,200.
      it.('it_swap_in_own_fee', 'diverges: D-TAX-IT-FEES') do |c|
        btc_on_binance.(c)
        price('BTC', '2026-06-01', '30000', 'EUR')
        price('ETH', '2026-06-01', '3000', 'EUR')
        price('ETH', '2026-06-05', '3000', 'EUR')
        row(c, :swap_out, 'BTC', '1', '2026-06-01T10:00:00Z', exchange: b.(c), group_id: 'cv1', tx_id: 'cv1-out-BTC')
        row(c, :swap_in, 'ETH', '10', '2026-06-01T10:00:00Z', exchange: b.(c), group_id: 'cv1', tx_id: 'cv1-in-ETH', fee_currency: 'ETH', fee_amount: '0.1')
        transfer(c, 'ETH', '9.9', '2026-06-05T10:00:00Z', from: b.(c), to: k.(c))
        kraken(c, :sell, 'ETH', '9.9', '30000', '2026-09-01T10:00:00Z')
      end,
      # As pt_swap_fee_exhausted: NOTE swap_fee_exhausted lists ETH 0.05 at 100.00 EUR; no disposal.
      it.('it_swap_fee_exhausted', 'diverges: D-TAX-SWAP-FEE-EXHAUSTED') do |c|
        price('BTC', '2026-02-01', '50000', 'EUR')
        price('ETH', '2026-02-01', '2000', 'EUR')
        quoted(c, :buy, 'BTC', '0.002', '100', '2026-01-05T10:00:00Z', fiat: 'EUR')
        quoted(c, :buy, 'ETH', '0.05', '0.002', '2026-02-01T10:00:00Z', fiat: 'BTC', fee_currency: 'ETH', fee_amount: '0.05')
      end,
      # Alpaca (USD at 1.17): the CFEE is no part of the buy's cost; it takes 0.01 of it at 100, no proceeds: 14,850 /
      # 9,900 / 4,950, tax_33 1,633.50.
      # Branch review: a CFEE of 2022 still folds into its buy's cost (fees were costs before 2023): 1 BTC for 10,500 USD
      # (10,000 at 1.05) less 0.01 → 0.99 at 10,000; sold 2026 for 17,374.50 USD (14,850) → 4,850, tax_33 1,600.50.
      it.('it_pre2023_cfee', 'diverges: D-TAX-IT-PRE2023-BASIS') do |c|
        price('BTC', '2022-06-01', '10000', 'EUR')
        buy(c, 'BTC', '1', '10500', '2022-06-01T15:00:00Z')
        row(c, :fee, 'BTC', '0.01', '2022-06-01T00:00:00Z', description: 'CFEE', raw_data: { 'activity_type' => 'CFEE' })
        sell(c, 'BTC', '0.99', '17374.50', '2026-03-10T10:00:00Z')
      end,
      # Branch review: a USDC fee row in a crypto-for-crypto swap's group is no exchange with an e-money token: no BTC row;
      # ETH (cost 20,000) sold for 33,000 → 13,000, tax_33 4,290.00.
      it.('it_emt_fee_row_not_leg', 'diverges: D-TAX-IT-SWAPS') do |c|
        btc_on_binance.(c)
        price('BTC', '2026-06-01', '30000', 'EUR')
        price('ETH', '2026-06-01', '2000', 'EUR')
        price('ETH', '2026-06-05', '2100', 'EUR')
        swap(c, 'BTC', '1', 'ETH', '15', '2026-06-01T10:00:00Z', ref: 'cv1')
        row(c, :fee, 'USDC', '5', '2026-06-01T10:00:00Z', exchange: b.(c), group_id: 'cv1', tx_id: 'cv1-fee-USDC')
        transfer(c, 'ETH', '15', '2026-06-05T10:00:00Z', from: b.(c), to: k.(c))
        kraken(c, :sell, 'ETH', '15', '33000', '2026-09-01T10:00:00Z')
      end,
      it.('it_cfee_into_buy', 'diverges: D-TAX-IT-FEES') do |c|
        price('BTC', '2026-01-10', '10000', 'EUR')
        buy(c, 'BTC', '1', '11700', '2026-01-10T10:00:00Z')
        row(c, :fee, 'BTC', '0.01', '2026-01-10T00:00:00Z', description: 'CFEE', raw_data: { 'activity_type' => 'CFEE' })
        sell(c, 'BTC', '0.99', '17374.50', '2026-03-10T10:00:00Z')
      end,
      # A sale's CFEE walks after it: 15,000 / 10,000 / 5,000, then the fee takes the last 0.01 at 100; tax_33 1,650.00.
      it.('it_cfee_into_sale', 'diverges: D-TAX-IT-FEES') do |c|
        price('BTC', '2026-03-10', '15000', 'EUR')
        buy(c, 'BTC', '1.01', '11817', '2026-01-10T10:00:00Z')
        sell(c, 'BTC', '1', '17550', '2026-03-10T10:00:00Z')
        row(c, :fee, 'BTC', '0.01', '2026-03-10T00:00:00Z', description: 'CFEE', raw_data: { 'activity_type' => 'CFEE' })
      end,
      # A swap of 2022 was an exchange (crypto was foreign currency): the 20 ETH cost their 60,000 value → 63,000 / 60,000 /
      # 3,000, tax_33 990.00 (a carry-over would give 43,000).
      it.('it_pre2023_swap_basis', 'diverges: D-TAX-IT-PRE2023-BASIS') do |c|
        price('BTC', '2021-07-01', '30000', 'EUR')
        price('BTC', '2022-06-01', '25000', 'EUR')
        price('ETH', '2022-06-01', '3000', 'EUR')
        price('ETH', '2022-07-01', '3100', 'EUR')
        kraken(c, :buy, 'BTC', '1', '20000', '2021-06-01T10:00:00Z')
        transfer(c, 'BTC', '1', '2021-07-01T10:00:00Z', from: k.(c), to: b.(c))
        swap(c, 'BTC', '1', 'ETH', '20', '2022-06-01T10:00:00Z', ref: 'cv1')
        transfer(c, 'ETH', '20', '2022-07-01T10:00:00Z', from: b.(c), to: k.(c))
        kraken(c, :sell, 'ETH', '20', '63000', '2026-03-01T10:00:00Z')
      end,
      # Outside the regime (2023–2026): refused as not ported.
      it.('it_2022_not_ported', 'parity', year: 2022, &two_years),
      it.('it_2027_not_ported', 'parity', year: 2027, &two_years)
    ]
  end

  # A12-4: Ireland by its own law (PLAN-A12 "A12-4", r0-r5; every figure computed by hand there). Dublin, EUR on Kraken.
  # The shared scenarios with a disposal show the summary and its hint (D-TAX-IE-SUMMARY); those without are Rails' file.
  IE_EXPECT = %w[ie_buy_sell_income_deposit_stated ie_missing_price ie_swap_chain ie_stablecoin_kept ie_stablecoin_as_fiat
                 ie_ecb_gap ie_oversold ie_lost ie_unlinked_withdrawal ie_coin_fee]
              .to_h { |name| [name, 'diverges: D-TAX-IE-SUMMARY'] }.freeze
  def ie_scenarios
    ie = ->(name, year: 2025, expect: 'diverges: D-TAX-IE-SUMMARY', &setup) { { name:, country: 'IE', year:, expect:, setup: ->(c) { user_zone(c, 'Dublin'); setup.(c) } } }
    # One coin bought and sold on Kraken for euros.
    trade = lambda do |c, coin, qty, cost, proceeds, bought, sold|
      kraken(c, :buy, coin, qty, cost, bought)
      kraken(c, :sell, coin, qty, proceeds, sold)
    end
    btc_gain = ->(c) { trade.(c, 'BTC', '1', '10000', '20000', '2024-02-01T10:00:00Z', '2025-03-01T10:00:00Z') }
    backward = lambda do |c|
      kraken(c, :buy, 'BTC', '2', '20000', '2023-01-10T10:00:00Z')
      kraken(c, :buy, 'BTC', '1', '40000', '2025-03-01T10:00:00Z')
      kraken(c, :sell, 'BTC', '1.5', '54000', '2025-03-20T10:00:00Z')
    end
    window_buys = lambda do |c|
      kraken(c, :buy, 'BTC', '1', '10000', '2023-01-10T10:00:00Z')
      kraken(c, :buy, 'BTC', '0.5', '15000', '2025-03-01T10:00:00Z')
      kraken(c, :buy, 'BTC', '0.5', '20000', '2025-03-10T10:00:00Z')
    end
    # +10,000 BTC (03-01); ETH 1 @ 4,000 (2024) sold 06-02 for 2,000; 1 ETH rebought for 2,100 on `rebuy`.
    fence = lambda do |c, rebuy, qty: '1', cost: '2100'|
      btc_gain.(c)
      trade.(c, 'ETH', '1', '4000', '2000', '2024-01-10T10:00:00Z', '2025-06-02T10:00:00Z')
      kraken(c, :buy, 'ETH', qty, cost, rebuy)
    end
    # ETH −2,000 on 2025-12-31, rebought 2026-01-20 for 2,100.
    into_january = lambda do |c|
      trade.(c, 'ETH', '1', '4000', '2000', '2024-01-10T10:00:00Z', '2025-12-31T10:00:00Z')
      kraken(c, :buy, 'ETH', '1', '2100', '2026-01-20T10:00:00Z')
    end
    two_losses = lambda do |c, rebought|
      btc_gain.(c)
      kraken(c, :buy, 'ETH', '1', '3000', '2024-01-10T10:00:00Z')
      kraken(c, :buy, 'ETH', '1', '2000', '2024-02-10T10:00:00Z')
      kraken(c, :sell, 'ETH', '1', '2000', '2025-05-01T10:00:00Z')
      kraken(c, :sell, 'ETH', '1', '1500', '2025-05-10T10:00:00Z')
      kraken(c, :buy, 'ETH', rebought, (rebought.to_d * 1800).to_s('F'), '2025-05-20T10:00:00Z')
    end
    day = lambda do |n, bought|
      ie.("ie_4week_day#{n}", expect: 'diverges: D-TAX-IE-4WEEK') do |c|
        kraken(c, :buy, 'ETH', '1', '2000', '2024-01-10T10:00:00Z')
        kraken(c, :buy, 'ETH', '1', '3000', "#{bought}T10:00:00Z")
        kraken(c, :sell, 'ETH', '1', '3500', '2025-06-02T10:00:00Z')
      end
    end
    ring = lambda do |n, rebuy|
      ie.("ie_ring_fence_day#{n}", expect: 'diverges: D-TAX-IE-RING-FENCE') { |c| fence.(c, "#{rebuy}T10:00:00Z") }
    end
    [
      # 54,000 / 45,000 / 9,000 → CGT 2,550.90 (Rails: the same row).
      ie.('ie_4week_backward', &backward),
      # Cost 20,000 (the 03-10 lot), −2,000 → CGT 0.00 (Rails: the 03-01 lot, +3,000).
      ie.('ie_4week_latest_first', expect: 'diverges: D-TAX-IE-4WEEK') { |c| window_buys.(c); kraken(c, :sell, 'BTC', '0.5', '18000', '2025-03-20T10:00:00Z') },
      # 20,000 + 15,000 + 0.2 × 10,000 = 37,000, gain 6,200 → taxable 4,930, CGT 1,626.90 (Rails: 22,000, 21,200).
      ie.('ie_4week_all_window_lots', expect: 'diverges: D-TAX-IE-4WEEK') { |c| window_buys.(c); kraken(c, :sell, 'BTC', '1.2', '43200', '2025-03-20T10:00:00Z') },
      # 500 (4_week_rule) / 500 (4_week_rule; Rails fifo 1,500) / 1,500 (fifo).
      day.(27, '2025-05-06'), day.(28, '2025-05-05'), day.(29, '2025-05-04'),
      # Day 27/28: restricted 2,000, losses 0.00, taxable 8,730, CGT 2,880.90; day 29: losses 2,000, CGT 2,220.90.
      ring.(27, '2025-06-29'), ring.(28, '2025-06-30'), ring.(29, '2025-07-01'),
      # Restricted 1,000, losses 1,000.00, CGT 2,550.90.
      ie.('ie_ring_fence_partial', expect: 'diverges: D-TAX-IE-RING-FENCE') { |c| fence.(c, '2025-06-12T10:00:00Z', qty: '0.5', cost: '1050') },
      # 5,000 / 2,100 / 2,900, released 2,000 → gains 12,900, total_losses 0.00, taxable 9,630, CGT 3,177.90.
      ie.('ie_ring_fence_released', expect: 'diverges: D-TAX-IE-RING-FENCE') do |c|
        fence.(c, '2025-06-29T10:00:00Z')
        kraken(c, :sell, 'ETH', '1', '5000', '2025-09-01T10:00:00Z')
      end,
      # Row 6,000 / 4,000 / 2,000: part A +2,000, part B 0 → released 0.00, not usable 2,000 → CGT 240.90.
      ie.('ie_release_mixed_parts', expect: 'diverges: D-TAX-IE-RING-FENCE') do |c|
        kraken(c, :buy, 'ETH', '1', '1000', '2024-01-10T10:00:00Z')
        kraken(c, :buy, 'ETH', '1', '5000', '2025-06-02T10:00:00Z')
        kraken(c, :sell, 'ETH', '1', '3000', '2025-06-02T10:00:30Z')
        kraken(c, :buy, 'ETH', '1', '3000', '2025-06-10T10:00:00Z')
        kraken(c, :sell, 'ETH', '2', '6000', '2025-09-01T10:00:00Z')
      end,
      # A row with a net gain opens no fence: CGT 2,550.90 unchanged.
      ie.('ie_fence_net_disposal', expect: 'diverges: D-TAX-IE-RING-FENCE') { |c| backward.(c); kraken(c, :buy, 'BTC', '1', '37000', '2025-04-01T10:00:00Z') },
      # Fences filled oldest first: restricted 1,500, CGT 2,880.90; with 1.5 ETH: restricted 1,250, losses 250.00, CGT 2,798.40.
      ie.('ie_fence_two_losses', expect: 'diverges: D-TAX-IE-RING-FENCE') { |c| two_losses.(c, '2') },
      ie.('ie_fence_two_losses_partial', expect: 'diverges: D-TAX-IE-RING-FENCE') { |c| two_losses.(c, '1.5') },
      # Day 28 across the 30 March clock change (10:00 Dublin = 09:00Z): restricted.
      ie.('ie_fence_dst', expect: 'diverges: D-TAX-IE-RING-FENCE') do |c|
        trade.(c, 'ETH', '1', '4000', '2000', '2024-01-10T10:00:00Z', '2025-03-03T10:00:00Z')
        kraken(c, :buy, 'ETH', '1', '2100', '2025-03-31T09:00:00Z')
      end,
      # The January lookahead: restricted 2,000 in the 2025 report; released 2,000 in 2026 (total_losses 0.00, CGT 0.00).
      ie.('ie_fence_into_january', expect: 'diverges: D-TAX-IE-RING-FENCE', &into_january),
      ie.('ie_fence_released_next_year', year: 2026, expect: 'diverges: D-TAX-IE-RING-FENCE') do |c|
        into_january.(c)
        kraken(c, :sell, 'ETH', '1', '4100', '2026-03-01T10:00:00Z')
      end,
      # The January rebuy (a swap) has no price: restricted as before, no banner, no warning line.
      ie.('ie_lookahead_unpriced', expect: 'diverges: D-TAX-IE-RING-FENCE') do |c|
        price('SOL', '2025-06-01', '150', 'EUR')
        kraken(c, :buy, 'SOL', '10', '1500', '2025-05-01T10:00:00Z')
        transfer(c, 'SOL', '10', '2025-06-01T10:00:00Z', from: venue(c, :kraken), to: venue(c, :binance))
        trade.(c, 'ETH', '1', '4000', '2000', '2024-01-10T10:00:00Z', '2025-12-31T10:00:00Z')
        swap(c, 'SOL', '10', 'ETH', '1', '2026-01-20T10:00:00Z', ref: 'jan')
      end,
      # −2,000 on 11-20, rebought 12-05, sold 12-20 +2,500 (released 2,000); +10,000 in May → Dec 15 2,880.90, year
      # 3,045.90, Jan 31 165.00.
      ie.('ie_fence_nov_dec', expect: 'diverges: D-TAX-IE-PERIODS') do |c|
        trade.(c, 'BTC', '1', '10000', '20000', '2024-02-01T10:00:00Z', '2025-05-10T10:00:00Z')
        trade.(c, 'ETH', '1', '4000', '2000', '2024-01-10T10:00:00Z', '2025-11-20T10:00:00Z')
        kraken(c, :buy, 'ETH', '1', '2000', '2025-12-05T10:00:00Z')
        kraken(c, :sell, 'ETH', '1', '4500', '2025-12-20T10:00:00Z')
      end,
      # A CFEE walked after its day's fills (New York's day differs) takes effect then: it is matched with the 01:00Z buy
      # it follows (s. 581(1)): 200 / 200 / 0, no loss → CGT 2,880.90.
      ie.('ie_cfee_fence', expect: 'diverges: D-TAX-IE-RING-FENCE') do |c|
        price('ETH', '2025-06-02', '2000', 'EUR')
        btc_gain.(c)
        kraken(c, :buy, 'ETH', '1', '4000', '2024-01-10T10:00:00Z')
        row(c, :fee, 'ETH', '0.1', '2025-06-02T00:00:00Z', description: 'CFEE', raw_data: { 'activity_type' => 'CFEE' })
        buy(c, 'ETH', '0.1', '220', '2025-06-02T01:00:00Z')
      end,
      # Branch review: the 01:00Z buy is sold at 02:00Z for 200 (0 gain); the CFEE then takes effect after both and comes
      # from the 2024 lot: 200 / 400 / −200, nothing bought after it → losses 200, CGT (10,000 − 200 − 1,270) × 33% = 2,814.90.
      ie.('ie_cfee_after_resale', expect: 'diverges: D-TAX-IE-RING-FENCE') do |c|
        price('ETH', '2025-06-02', '2000', 'EUR')
        btc_gain.(c)
        kraken(c, :buy, 'ETH', '1', '4000', '2024-01-10T10:00:00Z')
        row(c, :fee, 'ETH', '0.1', '2025-06-02T00:00:00Z', description: 'CFEE', raw_data: { 'activity_type' => 'CFEE' })
        buy(c, 'ETH', '0.1', '220', '2025-06-02T01:00:00Z')
        sell(c, 'ETH', '0.1', '220', '2025-06-02T02:00:00Z')
      end,
      # Branch review: a staking reward (0.2 ETH, 400) at 00:30Z, 0.1 sold at 01:00Z for 200 (0 gain); the CFEE after them is
      # matched with the reward's other 0.1 (s. 581(1): any acquisition of the 4 weeks before): 200 / 200 / 0 → CGT 2,880.90.
      ie.('ie_cfee_after_reward', expect: 'diverges: D-TAX-IE-RING-FENCE') do |c|
        price('ETH', '2025-06-02', '2000', 'EUR')
        btc_gain.(c)
        kraken(c, :buy, 'ETH', '1', '4000', '2024-01-10T10:00:00Z')
        row(c, :fee, 'ETH', '0.1', '2025-06-02T00:00:00Z', description: 'CFEE', raw_data: { 'activity_type' => 'CFEE' })
        row(c, :staking_reward, 'ETH', '0.2', '2025-06-02T00:30:00Z')
        sell(c, 'ETH', '0.1', '220', '2025-06-02T01:00:00Z')
      end,
      # Branch review: the rebuy at the very instant of the loss sale, booked after it → restricted 2,000 → CGT 2,880.90.
      ie.('ie_fence_same_instant', expect: 'diverges: D-TAX-IE-RING-FENCE') do |c|
        btc_gain.(c)
        kraken(c, :buy, 'ETH', '1', '4000', '2024-01-10T10:00:00Z')
        kraken(c, :sell, 'ETH', '1', '2000', '2025-06-02T10:00:00Z', ref: 'loss')
        kraken(c, :buy, 'ETH', '1', '2100', '2025-06-02T10:00:00Z', ref: 'rebuy')
      end,
      # Branch review: an unpriced deposit at 2026-01-01T00:00Z (midnight in Dublin) is 2026's: no banner, no deposit block
      # in the 2025 report → BTC +10,000, CGT 2,880.90.
      # Confirmation round: east of UTC (Tallinn) the year ends at local midnight, 2025-12-31T22:00Z: an unpriced deposit
      # then is 2026's → no banner, no deposit block; BTC +10,000, CGT 2,880.90.
      ie.('ie_new_year_east', expect: 'diverges: D-TAX-IE-SUMMARY') do |c|
        user_zone(c, 'Tallinn')
        btc_gain.(c)
        row(c, :deposit, 'SOL', '5', '2025-12-31T22:00:00Z', exchange: venue(c, :kraken))
      end,
      # Confirmation round: a tokenized asset bought at Tallinn's midnight (2025-12-31T22:00Z) is 2026's: the 2025 report is
      # made (Rails refuses it) → BTC +10,000, CGT 2,880.90.
      ie.('ie_tokenized_next_year', expect: 'diverges: D-TAX-IE-SUMMARY') do |c|
        user_zone(c, 'Tallinn')
        btc_gain.(c)
        coin = Asset.create!(external_id: 'tslax-coin', symbol: 'TSLAX', name: 'coin', category: 'Cryptocurrency')
        wrapper = Asset.create!(external_id: 'tesla-xstock', symbol: 'TSLAx', name: 'wrapper', category: 'Cryptocurrency', instrument_type: 'tokenized')
        ExchangeAsset.create!(exchange: c[:alpaca], asset: wrapper, available: true)
        Ticker.create!(exchange: c[:alpaca], ticker: 'TSLAX/USD', base: 'TSLAX', quote: 'USD', base_asset: wrapper, quote_asset: c[:assets]['USD'],
                       base_decimals: 9, quote_decimals: 2, price_decimals: 2, minimum_base_size: '0.000000001', minimum_quote_size: '1')
        coin.touch
        buy(c, 'TSLAX', '1', '300', '2025-12-31T22:00:00Z')
      end,
      ie.('ie_new_year_midnight', expect: 'diverges: D-TAX-IE-SUMMARY') do |c|
        btc_gain.(c)
        row(c, :deposit, 'SOL', '5', '2026-01-01T00:00:00Z', exchange: venue(c, :kraken))
      end,
      # A loss on 2026-09-20, its window open on the recording's day (2026-10-01): NOTE ie_provisional.
      ie.('ie_provisional_note', year: 2026) { |c| trade.(c, 'ETH', '1', '4000', '2000', '2025-01-10T10:00:00Z', '2026-09-20T10:00:00Z') },
      # Dec 15 2,880.90, Jan 31 1,650.00 (Rails 3,020.60 / 1,510.30).
      ie.('ie_periods_dec_gain', expect: 'diverges: D-TAX-IE-PERIODS') do |c|
        kraken(c, :buy, 'BTC', '2', '20000', '2024-02-01T10:00:00Z')
        kraken(c, :sell, 'BTC', '1', '20000', '2025-05-10T10:00:00Z')
        kraken(c, :sell, 'BTC', '1', '15000', '2025-12-10T10:00:00Z')
      end,
      # CGT 1,560.90: Dec 15 0.00, Jan 31 1,560.90 (Rails 260.15 / 1,300.75).
      ie.('ie_periods_loss_before_dec', expect: 'diverges: D-TAX-IE-PERIODS') do |c|
        trade.(c, 'BTC', '1', '10000', '12000', '2024-02-01T10:00:00Z', '2025-02-10T10:00:00Z')
        trade.(c, 'ETH', '1', '10000', '4000', '2024-02-01T10:00:00Z', '2025-06-10T10:00:00Z')
        trade.(c, 'SOL', '100', '5000', '15000', '2024-02-01T10:00:00Z', '2025-12-10T10:00:00Z')
      end,
      # CGT 1,560.90: Dec 15 2,880.90, Jan 31 0.00, overpaid 1,320.00.
      ie.('ie_periods_dec_loss', expect: 'diverges: D-TAX-IE-PERIODS') do |c|
        trade.(c, 'BTC', '1', '10000', '20000', '2024-02-01T10:00:00Z', '2025-05-10T10:00:00Z')
        trade.(c, 'ETH', '1', '10000', '6000', '2024-02-01T10:00:00Z', '2025-12-10T10:00:00Z')
      end,
      # cgt_33 900.90, cgt_31 1,240.00, total 2,140.90 (Rails 2,220.90).
      ie.('ie_rate_31', year: 2026, expect: 'diverges: D-TAX-IE-RATE') do |c|
        trade.(c, 'BTC', '1', '10000', '16000', '2025-02-01T10:00:00Z', '2026-03-01T10:00:00Z')
        trade.(c, 'ETH', '1', '4000', '2000', '2025-02-01T10:00:00Z', '2026-10-15T10:00:00Z')
        trade.(c, 'SOL', '100', '6000', '10000', '2025-02-01T10:00:00Z', '2026-11-02T10:00:00Z')
      end,
      # 23:30 Dublin on 6 October (33%) and 00:30 on 7 October (31%): cgt_33 1,230.90, cgt_31 1,550.00, total 2,780.90.
      ie.('ie_rate_boundary', year: 2026, expect: 'diverges: D-TAX-IE-RATE') do |c|
        kraken(c, :buy, 'BTC', '2', '20000', '2025-02-01T10:00:00Z')
        kraken(c, :sell, 'BTC', '1', '15000', '2026-10-06T22:30:00Z')
        kraken(c, :sell, 'BTC', '1', '15000', '2026-10-06T23:30:00Z')
      end,
      # A release on the 31% row: cgt_33 1,560.90, cgt_31 620.00, total 2,180.90.
      ie.('ie_rate_31_released', year: 2026, expect: 'diverges: D-TAX-IE-RATE') do |c|
        trade.(c, 'BTC', '1', '10000', '16000', '2025-02-01T10:00:00Z', '2026-03-01T10:00:00Z')
        trade.(c, 'ETH', '1', '4000', '2000', '2025-02-01T10:00:00Z', '2026-10-08T10:00:00Z')
        trade.(c, 'ETH', '1', '2000', '6000', '2026-10-20T10:00:00Z', '2026-11-10T10:00:00Z')
      end,
      # A release beyond the row's gain: row 0.00, a = −2,000 (31%) off the 33% gains → cgt_33 900.90, total 900.90.
      ie.('ie_release_overshoot', year: 2026, expect: 'diverges: D-TAX-IE-RING-FENCE') do |c|
        trade.(c, 'BTC', '1', '10000', '16000', '2025-02-01T10:00:00Z', '2026-03-01T10:00:00Z')
        kraken(c, :buy, 'ETH', '1', '5000', '2024-02-01T10:00:00Z')
        kraken(c, :buy, 'ETH', '1', '3000', '2026-10-01T10:00:00Z')
        kraken(c, :sell, 'ETH', '1', '1000', '2026-10-01T10:00:30Z')
        kraken(c, :buy, 'ETH', '1', '1000', '2026-10-10T10:00:00Z')
        kraken(c, :sell, 'ETH', '2', '6000', '2026-11-20T10:00:00Z')
      end,
      # 0.1 BTC (BTC 50,000) → 33.3 SOL (150): 4,995.00 / 3,000.00 / 1,995.00 → CGT 239.25 (Rails: 5,000 / 2,000).
      ie.('ie_swap_value_received', year: 2026, expect: 'diverges: D-TAX-SWAP-RECEIVED') do |c|
        price('BTC', '2025-06-01', '60000', 'EUR')
        price('BTC', '2026-03-01', '50000', 'EUR')
        price('SOL', '2026-03-01', '150', 'EUR')
        kraken(c, :buy, 'BTC', '1', '30000', '2025-05-01T10:00:00Z')
        transfer(c, 'BTC', '1', '2025-06-01T10:00:00Z', from: venue(c, :kraken), to: venue(c, :binance))
        swap(c, 'BTC', '0.1', 'SOL', '33.3', '2026-03-01T10:00:00Z', ref: 'sv')
      end,
      # Staking only: no summary, Rails' file.
      ie.('ie_income_only', year: 2026, expect: 'parity') do |c|
        price('ETH', '2026-03-01', '2000', 'EUR')
        row(c, :staking_reward, 'ETH', '0.1', '2026-03-01T00:00:00Z', exchange: venue(c, :kraken))
      end,
      # D-TAX-IE-YEARS: 2013–2026 only.
      ie.('ie_2012_not_ported', year: 2012, expect: 'parity') { |c| trade.(c, 'BTC', '1', '100', '200', '2012-02-01T10:00:00Z', '2012-03-01T10:00:00Z') },
      ie.('ie_2027_not_ported', year: 2027, expect: 'parity') { |c| trade.(c, 'BTC', '1', '100', '200', '2026-02-01T10:00:00Z', '2027-03-01T10:00:00Z') }
    ]
  end

  # Poland's own vectors state their NBP rows (none by default), so the shared scenarios' default rates never reach them.
  def pl_case(name, year: 2026, expect: 'diverges: D-TAX-PL-COST-POOL', nbp: [], nbp_coverage: [], **opts, &setup)
    { name:, country: 'PL', year:, expect:, nbp:, nbp_coverage:, **opts, setup: lambda { |c|
      user_zone(c, 'Warsaw')
      setup.(c)
    } }
  end

  # NBP table A for a PL scenario that states none (the shared ones), as `ecb` gives every scenario the ECB's: USD and
  # EUR on every weekday from a week before its first row to its last, each currency covered over that span.
  NBP_DEFAULT = { 'USD' => '3.9000', 'EUR' => '4.3000' }.freeze
  def pl_default_nbp(scenario)
    return [scenario.fetch(:nbp, []), scenario.fetch(:nbp_coverage, [])] if scenario[:country] != 'PL' || scenario.key?(:nbp)
    days = AccountTransaction.pluck(:transacted_at).map { |t| t.in_time_zone('Warsaw').to_date }
    return [[], []] if days.empty?
    span = (days.min - 7)..days.max
    weekdays = span.reject { |d| d.saturday? || d.sunday? }
    [NBP_DEFAULT.flat_map { |cur, rate| weekdays.map { |d| [cur, d.iso8601, rate] } },
     NBP_DEFAULT.keys.map { |cur| [cur, span.first.iso8601, span.last.iso8601] }]
  end

  # A Bybit row quoted in cash: one row per trade.
  def quoted(ctx, side, coin, qty, cash, at, fiat: 'PLN', **attrs)
    ctx[:bybit] ||= Exchanges::Bybit.create!(name: 'Bybit', maker_fee: '0.1', taker_fee: '0.1')
    row(ctx, side, coin, qty, at, exchange: ctx[:bybit], quote_currency: fiat, quote_amount: cash, **attrs)
  end


  # A12-3: Bulgaria by its own law (PLAN-A12 "A12-3", r0-r3; every figure computed by hand there). EUR from 2026, BGN up to
  # 2025 (EUR × 1.95583 exactly; another currency at 1.95583 / ECB rounded to six significant digits). Sofia.
  BG_EXPECT = %w[bg_buy_sell_income_deposit_stated bg_missing_price bg_swap_chain bg_stablecoin_kept bg_stablecoin_as_fiat
                 bg_ecb_gap bg_oversold bg_lost bg_unlinked_withdrawal bg_coin_fee]
              .to_h { |name| [name, 'diverges: D-TAX-BG-ANNUAL'] }.merge('bg_prior_year_only' => 'parity').freeze
  def bg_scenarios
    bg = ->(name, year: 2026, expect: 'diverges: D-TAX-BG-ANNUAL', &setup) { { name:, country: 'BG', year:, expect:, setup: ->(c) { user_zone(c, 'Sofia'); setup.(c) } } }
    [
      # Average 35,000 EUR: BGN 97,791.50 / 68,454.05 / 29,337.45; ред 6 2,933.75, ред 7 26,403.70, tax 2,640.37.
      bg.('bg_average_2025_bgn', year: 2025, expect: 'diverges: D-TAX-BG-AVERAGE') do |c|
        kraken(c, :buy, 'BTC', '1', '30000', '2025-03-01T10:00:00Z')
        kraken(c, :buy, 'BTC', '1', '40000', '2025-04-01T10:00:00Z')
        kraken(c, :sell, 'BTC', '1', '50000', '2025-06-02T10:00:00Z')
      end,
      # +20,000 and −11,000 EUR: к.5 39,116.60, к.6 21,514.13, ред 5 17,602.47, ред 6 1,760.25, ред 7 15,842.22, tax 1,584.22.
      bg.('bg_netting_once', year: 2025) do |c|
        kraken(c, :buy, 'BTC', '1', '10000', '2025-02-03T10:00:00Z')
        kraken(c, :buy, 'ETH', '1', '20000', '2025-02-03T11:00:00Z')
        kraken(c, :sell, 'BTC', '1', '30000', '2025-06-02T10:00:00Z')
        kraken(c, :sell, 'ETH', '1', '9000', '2025-06-02T11:00:00Z')
      end,
      # 2023: gain 9,779.15 BGN, no ред 6, ред 7 9,779.15, tax 977.92; heading without row and code.
      bg.('bg_before_2024', year: 2023) do |c|
        kraken(c, :buy, 'BTC', '1', '10000', '2023-02-01T10:00:00Z')
        kraken(c, :sell, 'BTC', '1', '15000', '2023-06-01T10:00:00Z')
      end,
      # BNB's USD rate 1.95583 / 1.1 = 1.77803: 97,791.65 / 58,674.99 / 39,116.66; ред 6 3,911.67, ред 7 35,204.99, tax 3,520.50.
      bg.('bg_usd_trade_bgn', year: 2025, expect: 'diverges: D-TAX-BG-CURRENCY') do |c|
        buy(c, 'BTC', '1', '33000', '2025-03-03T10:00:00Z')
        sell(c, 'BTC', '1', '55000', '2025-06-02T10:00:00Z')
      end,
      # Swap 1 BTC (45,000) for 20 ETH at the average 35,000: 45,000 / 35,000 / 10,000; ETH sold for 47,000: 2,000.
      # ред 5 12,000, ред 6 1,200, ред 7 10,800, tax 1,080.
      bg.('bg_swap_average', expect: 'diverges: D-TAX-BG-AVERAGE') do |c|
        price('BTC', '2026-01-05', '44000', 'EUR')
        price('BTC', '2026-02-02', '45000', 'EUR')
        price('ETH', '2026-02-02', '2250', 'EUR')
        price('ETH', '2026-02-03', '2300', 'EUR')
        kraken(c, :buy, 'BTC', '1', '30000', '2025-03-03T10:00:00Z')
        kraken(c, :buy, 'BTC', '1', '40000', '2025-04-01T10:00:00Z')
        transfer(c, 'BTC', '1', '2026-01-05T10:00:00Z', from: venue(c, :kraken), to: venue(c, :binance))
        swap(c, 'BTC', '1', 'ETH', '20', '2026-02-02T10:00:00Z', ref: 'cv1')
        transfer(c, 'ETH', '20', '2026-02-03T10:00:00Z', from: venue(c, :binance), to: venue(c, :kraken))
        kraken(c, :sell, 'ETH', '20', '47000', '2026-03-02T10:00:00Z')
      end,
      # An unlinked deposit opens at cost 0; its stated 2,340 USD (= 2,000 EUR) is listed, not its cost: 2,500 / 0 / 2,500.
      bg.('bg_deposit_zero_stated', expect: 'diverges: D-TAX-BG-DEPOSIT-ZERO') do |c|
        row(c, :deposit, 'ETH', '1', '2026-02-02T10:00:00Z', exchange: venue(c, :kraken), manual_values: { 'price' => '2340' })
        kraken(c, :sell, 'ETH', '1', '2500', '2026-03-02T10:00:00Z')
      end,
      # D-TAX-SWAP-RECEIVED: 0.1 BTC (5,000) for 33.3 SOL (4,995): 4,995.00 / 3,000.00 / 1,995.00; SOL costs 4,995.
      bg.('bg_swap_unequal_legs', expect: 'diverges: D-TAX-SWAP-RECEIVED') do |c|
        price('BTC', '2025-06-02', '35000', 'EUR')
        price('BTC', '2026-02-02', '50000', 'EUR')
        price('SOL', '2026-02-02', '150', 'EUR')
        kraken(c, :buy, 'BTC', '1', '30000', '2025-03-03T10:00:00Z')
        transfer(c, 'BTC', '1', '2025-06-02T10:00:00Z', from: venue(c, :kraken), to: venue(c, :binance))
        swap(c, 'BTC', '0.1', 'SOL', '33.3', '2026-02-02T10:00:00Z', ref: 'cv1')
      end,
      # Branch review: the coin received has no price. The BTC keeps its own value as the estimate (5,000.00 / 3,000.00 /
      # 2,000.00) and is incomplete; the banner counts the missing price.
      bg.('bg_swap_received_unpriced', expect: 'diverges: D-TAX-SWAP-RECEIVED') do |c|
        price('BTC', '2025-06-02', '35000', 'EUR')
        price('BTC', '2026-02-02', '50000', 'EUR')
        kraken(c, :buy, 'BTC', '1', '30000', '2025-03-03T10:00:00Z')
        transfer(c, 'BTC', '1', '2025-06-02T10:00:00Z', from: venue(c, :kraken), to: venue(c, :binance))
        swap(c, 'BTC', '0.1', 'PEPE', '1000000', '2026-02-02T10:00:00Z', ref: 'cv1')
      end,
      # Branch review: a fee row booked in the swap's group is no leg. The swap is valued at the SOL received (4,995.00 /
      # 3,000.00 / 1,995.00) and the 0.001 BTC fee is its own disposal (50.00 / 30.00 / 20.00) → к.3 5,045.00, к.4 3,030.00,
      # к.5 2,015.00, ред 6 201.50, ред 7 1,813.50, tax 181.35.
      bg.('bg_swap_with_fee_row', expect: 'diverges: D-TAX-SWAP-RECEIVED') do |c|
        price('BTC', '2025-06-02', '35000', 'EUR')
        price('BTC', '2026-02-02', '50000', 'EUR')
        price('SOL', '2026-02-02', '150', 'EUR')
        kraken(c, :buy, 'BTC', '1', '30000', '2025-03-03T10:00:00Z')
        transfer(c, 'BTC', '1', '2025-06-02T10:00:00Z', from: venue(c, :kraken), to: venue(c, :binance))
        swap(c, 'BTC', '0.1', 'SOL', '33.3', '2026-02-02T10:00:00Z', ref: 'cv1')
        row(c, :fee, 'BTC', '0.001', '2026-02-02T10:00:00Z', exchange: venue(c, :binance), group_id: 'cv1')
      end,
      # Bought 2025 for 58,674.90 BGN, sold 2026 for 40,000 EUR: cost 30,000.00, gain 10,000.00.
      bg.('bg_bgn_leg_2026', expect: 'diverges: D-TAX-BG-CURRENCY') do |c|
        quoted(c, :buy, 'BTC', '1', '58674.90', '2025-06-02T10:00:00Z', fiat: 'BGN')
        quoted(c, :sell, 'BTC', '1', '40000', '2026-03-02T10:00:00Z', fiat: 'EUR')
      end,
      # +1,000 and −3,000 EUR: к.5 1,955.83, к.6 5,867.49, ред 5 0, ред 6 0, ред 7 0, tax 0.
      bg.('bg_net_loss', year: 2025) do |c|
        kraken(c, :buy, 'BTC', '1', '10000', '2025-02-03T10:00:00Z')
        kraken(c, :buy, 'ETH', '1', '5000', '2025-02-03T11:00:00Z')
        kraken(c, :sell, 'BTC', '1', '11000', '2025-06-02T10:00:00Z')
        kraken(c, :sell, 'ETH', '1', '2000', '2025-06-02T11:00:00Z')
      end
    ]
  end

  # A12-3: the Czech Republic by its own law (PLAN-A12 "A12-3", r0-r3). CZK at the ČNB fixing of the day (Rust: the `cnb`
  # rows; Rails: ECB CZK 25.0 and CZK prices = EUR × 25, `cz_prices`). Prague.
  CZ_EXPECT = %w[cz_buy_sell_income_deposit_stated cz_missing_price cz_swap_chain cz_stablecoin_kept cz_stablecoin_as_fiat
                 cz_oversold cz_lost cz_unlinked_withdrawal cz_coin_fee]
              .to_h { |name| [name, 'diverges: D-TAX-CZ-PRILOHA2'] }
              .merge('cz_prior_year_only' => 'parity', 'cz_ecb_gap' => 'diverges: D-TAX-CZ-CNB').freeze
  def cz_scenarios
    cz = lambda do |name, year: 2026, expect: 'diverges: D-TAX-CZ-PRILOHA2', **opts, &setup|
      { name:, country: 'CZ', year:, expect:, **opts, setup: ->(c) { user_zone(c, 'Prague'); setup.(c) } }
    end
    k = ->(c, side, coin, qty, cash, at, **o) { kraken(c, side, coin, qty, cash, at, fiat: 'CZK', **o) }
    # ETH bought 2024 for 40,000, sold 2025-01-20 for 50,000 (before the window); SOL bought 2024 for 50,000 and swapped
    # 2025-04-10 for 1 BTC worth 60,000; 0.5 BTC sold 2025-09-01 for `september`.
    window = lambda do |c, september|
      price('SOL', '2025-03-03', '2000', 'EUR')
      price('SOL', '2025-04-10', '2400', 'EUR')
      price('BTC', '2025-04-10', '2400', 'EUR')
      price('BTC', '2025-04-11', '2400', 'EUR')
      k.(c, :buy, 'ETH', '1', '40000', '2024-03-04T10:00:00Z')
      k.(c, :buy, 'SOL', '1', '50000', '2024-03-04T11:00:00Z')
      k.(c, :sell, 'ETH', '1', '50000', '2025-01-20T10:00:00Z')
      transfer(c, 'SOL', '1', '2025-03-03T10:00:00Z', from: venue(c, :kraken), to: venue(c, :binance))
      swap(c, 'SOL', '1', 'BTC', '1', '2025-04-10T10:00:00Z', ref: 'cv1')
      transfer(c, 'BTC', '1', '2025-04-11T10:00:00Z', from: venue(c, :binance), to: venue(c, :kraken))
      k.(c, :sell, 'BTC', '0.5', september, '2025-09-01T10:00:00Z')
    end
    # 1 ETH bought for 100,000 CZK on `bought`, swapped 2026-03-02 for 2 SOL worth 200,000 with a 2,000 CZK fee on the
    # in-leg; the SOL sold 2026-04-01 for 210,000 (its basis 200,000).
    swap_fee = lambda do |c, bought|
      price('ETH', '2026-01-12', '4000', 'EUR')
      price('ETH', '2026-03-02', '8000', 'EUR')
      price('SOL', '2026-03-02', '4000', 'EUR')
      price('SOL', '2026-03-03', '4000', 'EUR')
      k.(c, :buy, 'ETH', '1', '100000', bought)
      transfer(c, 'ETH', '1', '2026-01-12T10:00:00Z', from: venue(c, :kraken), to: venue(c, :binance))
      row(c, :swap_out, 'ETH', '1', '2026-03-02T10:00:00Z', exchange: venue(c, :binance), group_id: 'cv1', tx_id: 'cv1-out')
      row(c, :swap_in, 'SOL', '2', '2026-03-02T10:00:00Z', exchange: venue(c, :binance), group_id: 'cv1', tx_id: 'cv1-in',
                                                          fee_currency: 'CZK', fee_amount: '2000')
      transfer(c, 'SOL', '2', '2026-03-03T10:00:00Z', from: venue(c, :binance), to: venue(c, :kraken))
      k.(c, :sell, 'SOL', '2', '210000', '2026-04-01T10:00:00Z')
    end
    alpaca_weekend = lambda do |c|
      buy(c, 'BTC', '0.01', '800', '2026-01-05T10:00:00Z')
      sell(c, 'BTC', '0.01', '1000', '2026-03-07T10:00:00Z')
    end
    boundary = lambda do |proceeds|
      lambda do |c|
        k.(c, :buy, 'BTC', '1', '50000', '2026-01-12T10:00:00Z')
        k.(c, :sell, 'BTC', '1', proceeds, '2026-03-02T10:00:00Z')
      end
    end
    [
      # Exempt row (časový test) 2,250,000 / 600,000; taxable row 2,250,000 / 1,000,000 / 1,250,000; value test 4,500,000
      # fails; ř. 207 2,250,000, ř. 208 1,000,000, ř. 209 1,250,000.
      cz.('cz_time_test') do |c|
        kraken(c, :buy, 'BTC', '1', '24000', '2022-01-10T10:00:00Z')
        kraken(c, :buy, 'BTC', '1', '40000', '2024-02-01T10:00:00Z')
        kraken(c, :sell, 'BTC', '2', '180000', '2026-03-02T10:00:00Z')
      end,
      # 2025-01-20: taxable 500,000 / 125,000 / 375,000; 2025-03-01: časový test; the window's 500,000 fails.
      cz.('cz_before_in_force', year: 2025) do |c|
        kraken(c, :buy, 'BTC', '1', '10000', '2021-01-10T10:00:00Z')
        kraken(c, :sell, 'BTC', '0.5', '20000', '2025-01-20T10:00:00Z')
        kraken(c, :sell, 'BTC', '0.5', '20000', '2025-03-01T10:00:00Z')
      end,
      # Window 60,000 + 30,000 = 90,000: both hodnotový test; ř. 207 50,000, ř. 208 40,000, ř. 209 10,000.
      cz.('cz_value_test_window', year: 2025) { |c| window.(c, '30000') },
      # Window 105,000: all taxable; ř. 207 155,000, ř. 208 120,000, ř. 209 35,000.
      cz.('cz_value_test_fails', year: 2025) { |c| window.(c, '45000') },
      # A time-test sale of 80,000 and a taxable one of 30,000 (cost 20,000): 110,000 fails; ř. 207 30,000, ř. 208 20,000.
      cz.('cz_value_test_counts_time_exempt') do |c|
        k.(c, :buy, 'ETH', '1', '10000', '2022-01-10T10:00:00Z')
        k.(c, :buy, 'SOL', '1', '20000', '2026-01-12T10:00:00Z')
        k.(c, :sell, 'ETH', '1', '80000', '2026-03-02T10:00:00Z')
        k.(c, :sell, 'SOL', '1', '30000', '2026-03-03T10:00:00Z')
      end,
      # USDC (an e-money token) outside the value test: BTC's 30,000 passes it; USDC taxable: ř. 207 80,000, ř. 208 75,000.
      cz.('cz_emt_outside_value_test', expect: 'diverges: D-TAX-CZ-EMT') do |c|
        k.(c, :buy, 'USDC', '3000', '75000', '2026-01-12T10:00:00Z')
        k.(c, :buy, 'BTC', '0.01', '20000', '2026-01-12T11:00:00Z')
        k.(c, :sell, 'USDC', '3000', '80000', '2026-03-02T10:00:00Z')
        k.(c, :sell, 'BTC', '0.01', '30000', '2026-03-02T11:00:00Z')
      end,
      # zk-exempt 50,000,000 / 10,000,000: share 20%; ř. 207 10,000,000, ř. 208 2,000,000, ř. 209 8,000,000; § 38v lists it.
      cz.('cz_cap_40m', expect: 'diverges: D-TAX-CZ-CAP') do |c|
        kraken(c, :buy, 'BTC', '25', '400000', '2021-01-10T10:00:00Z')
        kraken(c, :sell, 'BTC', '25', '2000000', '2026-03-02T10:00:00Z')
      end,
      # Ten time-test sales of 5,500,000: S 55,000,000, share 15/55; each exempt part 4,000,000: no § 38v line.
      cz.('cz_cap_38v', expect: 'diverges: D-TAX-CZ-CAP') do |c|
        k.(c, :buy, 'BTC', '10', '10000000', '2021-01-11T10:00:00Z')
        (2..11).each { |d| k.(c, :sell, 'BTC', '1', '5500000', format('2026-03-%02dT10:00:00Z', d)) }
      end,
      # Branch review: two sales with no transaction id in the same second on one venue are two receipts. Each
      # 3,000,000 / 250,000 / 2,750,000, časový test; value test 6,000,000 fails; ř. 207–209 0; no § 38v line (each
      # exempt receipt 3,000,000 ≤ 5,000,000).
      cz.('cz_38v_same_second', expect: 'diverges: D-TAX-CZ-PRILOHA2') do |c|
        quoted(c, :buy, 'BTC', '2', '500000', '2021-01-11T10:00:00Z', fiat: 'CZK')
        2.times { quoted(c, :sell, 'BTC', '1', '3000000', '2026-03-02T10:00:00Z', fiat: 'CZK', tx_id: nil) }
      end,
      # Mining at cost 0 and no income: 250,000 / 0 / 250,000; NOTE.
      cz.('cz_mining_zero_cost', expect: 'diverges: D-TAX-CZ-MINING') do |c|
        price('BTC', '2026-02-02', '90000', 'EUR')
        row(c, :mining, 'BTC', '0.1', '2026-02-02T10:00:00Z', exchange: venue(c, :kraken))
        k.(c, :sell, 'BTC', '0.1', '250000', '2026-03-02T10:00:00Z')
      end,
      # A Saturday sale of 1,000 USD at Friday's 21.368: 21,368.00.
      cz.('cz_cnb_weekend', expect: 'diverges: D-TAX-CZ-CNB', &alpaca_weekend),
      # Coverage ends Thursday: the Saturday sale is missing, 0.00, with the banner.
      cz.('cz_cnb_gap', expect: 'diverges: D-TAX-CZ-CNB', cnb_coverage: [%w[2025-12-29 2026-03-05]], &alpaca_weekend),
      # A grouped USD sale (Kraken) that Saturday, coverage ending Thursday: missing, 0.00, banner; the coin's EUR price of
      # the day is not put in its place (r2 item 6).
      cz.('cz_cnb_gap_grouped', expect: 'diverges: D-TAX-CZ-CNB', cnb_coverage: [%w[2025-12-29 2026-03-05]]) do |c|
        price('BTC', '2026-03-07', '85000', 'EUR')
        kraken(c, :buy, 'BTC', '0.01', '800', '2026-01-05T10:00:00Z', fiat: 'USD')
        kraken(c, :sell, 'BTC', '0.01', '1000', '2026-03-07T10:00:00Z', fiat: 'USD')
      end,
      # CZK-quoted rows: 1,200,000 / 1,000,000 / 200,000 exactly.
      cz.('cz_czk_leg') do |c|
        quoted(c, :buy, 'BTC', '1', '1000000', '2026-01-12T10:00:00Z', fiat: 'CZK')
        quoted(c, :sell, 'BTC', '1', '1200000', '2026-03-02T10:00:00Z', fiat: 'CZK')
      end,
      # D-TAX-SWAP-RECEIVED: 124,875 / 75,000 / 49,875.
      cz.('cz_swap_unequal_legs', expect: 'diverges: D-TAX-SWAP-RECEIVED') do |c|
        price('BTC', '2025-06-02', '35000', 'EUR')
        price('BTC', '2026-02-02', '50000', 'EUR')
        price('SOL', '2026-02-02', '150', 'EUR')
        kraken(c, :buy, 'BTC', '1', '30000', '2025-03-03T10:00:00Z')
        transfer(c, 'BTC', '1', '2025-06-02T10:00:00Z', from: venue(c, :kraken), to: venue(c, :binance))
        swap(c, 'BTC', '0.1', 'SOL', '33.3', '2026-02-02T10:00:00Z', ref: 'cv1')
      end,
      # Exactly 100,000.00: hodnotový test, ř. 207 0.
      cz.('cz_value_test_boundary', &boundary.('100000.00')),
      # 100,000.01: taxable; ř. 207 100,000.01, ř. 208 50,000, ř. 209 50,000.01.
      cz.('cz_value_test_boundary_over', &boundary.('100000.01')),
      # Sold 150,000, cost 180,000: ř. 207 150,000, ř. 208 150,000 (capped), ř. 209 0.
      cz.('cz_net_loss') do |c|
        k.(c, :buy, 'BTC', '1', '180000', '2026-01-12T10:00:00Z')
        k.(c, :sell, 'BTC', '1', '150000', '2026-03-02T10:00:00Z')
      end,
      # A time-test sale 300,000 fee 300 (no expense) and a taxable one 200,000 / 150,000 fee 200: ř. 207 200,000, ř. 208
      # 150,200, ř. 209 49,800.
      cz.('cz_mixed_exempt_fees') do |c|
        k.(c, :buy, 'ETH', '1', '50000', '2022-01-10T10:00:00Z')
        k.(c, :buy, 'BTC', '1', '150000', '2026-01-12T10:00:00Z')
        k.(c, :sell, 'ETH', '1', '300000', '2026-03-02T10:00:00Z', fee: '300')
        k.(c, :sell, 'BTC', '1', '200000', '2026-03-03T10:00:00Z', fee: '200')
      end,
      # D-TAX-CZ-SWAP-FEE: 200,000 / 100,000 / fee 2,000 / 98,000; SOL 210,000 / 200,000 / 10,000.
      cz.('cz_swap_fee_in_leg', expect: 'diverges: D-TAX-CZ-SWAP-FEE') { |c| swap_fee.(c, '2026-01-10T10:00:00Z') },
      # Held since 2022: the swap is časový test, its fee no expense; SOL 210,000 / 200,000 / 10,000.
      cz.('cz_swap_fee_exempt', expect: 'diverges: D-TAX-CZ-SWAP-FEE') { |c| swap_fee.(c, '2022-01-10T10:00:00Z') }
    ]
  end

  # Rails' CZK prices: every EUR price × 25 (the ECB CZK rate the CZ scenarios give it).
  def cz_prices
    HistoricalPrice.where(currency: 'EUR').find_each do |p|
      HistoricalPrice.create!(asset: p.asset, currency: 'CZK', date: p.date, price: p.price * 25)
    end
  end

  # The ČNB fixings a CZ scenario states (Rails has no ČNB table): by default EUR 25.000 and USD 25 / the ECB's USD rate
  # of the year (22.727 in 2025, 21.368 in 2026) on the weekdays of the three days up to each row's Prague day, covered
  # from a week before the first row to the end of the report year. `cnb` rows: [currency, date, rate per unit];
  # `cnb_coverage`: [from, to].
  def cz_default_cnb(scenario)
    return [[], []] if scenario[:country] != 'CZ'
    days = AccountTransaction.pluck(:transacted_at).map { |t| t.in_time_zone('Prague').to_date }
    return [[], scenario.fetch(:cnb_coverage, [])] if days.empty?
    fixing = days.flat_map { |d| (d - 3..d).to_a }.uniq.sort.reject { |d| d.saturday? || d.sunday? }
    rates = fixing.flat_map do |d|
      [['EUR', d.iso8601, '25.000'], ['USD', d.iso8601, format('%.3f', 25 / USD_PER_EUR.fetch(d.year, '1.1').to_d)]]
    end
    year_end = Date.new([scenario.fetch(:year, 2026), days.max.year].max, 12, 31)
    [rates, scenario.fetch(:cnb_coverage, [[(days.min - 7).iso8601, year_end.iso8601]])]
  end

  def fifo_jurisdictions
    eur = lambda do |c|
      price('BTC', '2025-03-01', '60000', 'EUR')
      price('ETH', '2026-02-01', '2800', 'EUR')
      price('SOL', '2026-02-05', '140', 'EUR')
      price('BTC', '2026-02-10', '90000', 'EUR')
      price('SOL', '2026-04-05', '190', 'EUR')
      kraken(c, :buy, 'BTC', '1', '60000', '2025-03-01T10:00:00Z', fee: '40')
      kraken(c, :buy, 'BTC', '0.5', '40000', '2026-01-10T10:00:00Z', fee: '20')
      row(c, :staking_reward, 'ETH', '0.01', '2026-02-01T00:00:00Z', exchange: venue(c, :kraken))
      row(c, :airdrop, 'SOL', '3', '2026-02-05T00:00:00Z', exchange: venue(c, :kraken), manual_values: { 'price' => '150.5' })
      row(c, :deposit, 'BTC', '0.1', '2026-02-10T08:00:00Z', exchange: venue(c, :kraken))
      kraken(c, :sell, 'BTC', '1.2', '108000', '2026-04-01T10:00:00Z', fee: '50')
      transfer(c, 'SOL', '3', '2026-04-05T08:00:00Z', from: venue(c, :kraken), to: c[:alpaca])
      sell(c, 'SOL', '3', '600', '2026-04-10T08:00:00Z')
    end
    missing = lambda do |c|
      kraken(c, :buy, 'BTC', '1', '60000', '2026-01-10T10:00:00Z')
      row(c, :deposit, 'ETH', '2', '2026-02-10T08:00:00Z', exchange: venue(c, :kraken))
      kraken(c, :sell, 'ETH', '2', '5000', '2026-03-10T08:00:00Z')
      kraken(c, :sell, 'BTC', '0.5', '45000', '2026-03-10T09:00:00Z')
    end
    # A Binance Convert ETH -> BTC, then the BTC sold for euros on Kraken: a chain where swaps are not taxable. The coins
    # move between the venues by linked transfers (Germany keeps lots per wallet).
    chain = lambda do |c|
      price('ETH', '2025-02-01', '3000', 'EUR')
      price('BTC', '2025-02-01', '90000', 'EUR')
      price('SOL', '2025-08-01', '150', 'EUR')
      price('BTC', '2025-08-01', '100000', 'EUR')
      price('ETH', '2024-07-01', '2100', 'EUR')
      price('BTC', '2025-09-01', '101000', 'EUR')
      price('SOL', '2025-09-02', '151', 'EUR')
      kraken(c, :buy, 'ETH', '3', '6000', '2024-06-01T10:00:00Z')
      transfer(c, 'ETH', '3', '2024-07-01T10:00:00Z', from: venue(c, :kraken), to: venue(c, :binance))
      swap(c, 'ETH', '3', 'BTC', '0.1', '2025-02-01T10:00:00Z', ref: 'cv1')
      swap(c, 'BTC', '0.05', 'SOL', '33.3', '2025-08-01T10:00:00Z', ref: 'cv2')
      transfer(c, 'BTC', '0.05', '2025-09-01T10:00:00Z', from: venue(c, :binance), to: venue(c, :kraken))
      transfer(c, 'SOL', '33.3', '2025-09-02T10:00:00Z', from: venue(c, :binance), to: venue(c, :kraken))
      kraken(c, :sell, 'BTC', '0.05', '5500', '2026-03-01T10:00:00Z')
      kraken(c, :sell, 'SOL', '33.3', '5328', '2026-03-02T10:00:00Z')
    end
    # BTC into USDT on Binance, the USDT sold for euros on Kraken: with and without the stablecoin flag.
    stable = lambda do |c|
      price('BTC', '2026-02-01', '90000', 'EUR')
      price('USDT', '2026-02-01', '0.86', 'EUR')
      price('BTC', '2024-07-01', '41000', 'EUR')
      kraken(c, :buy, 'BTC', '1', '40000', '2024-06-01T10:00:00Z')
      transfer(c, 'BTC', '1', '2024-07-01T10:00:00Z', from: venue(c, :kraken), to: venue(c, :binance))
      swap(c, 'BTC', '0.5', 'USDT', '52000', '2026-02-01T10:00:00Z', ref: 'cv1')
      transfer(c, 'USDT', '52000', '2026-02-01T12:00:00Z', from: venue(c, :binance), to: venue(c, :kraken))
      kraken(c, :sell, 'USDT', '52000', '44700', '2026-02-02T10:00:00Z')
    end
    # A USD-quoted Alpaca sale on a day the ECB has no rate for, a week either way: the BTC cross.
    gap = lambda do |c|
      price('BTC', '2026-05-15', '100000')
      price('BTC', '2026-05-15', '86000', 'EUR')
      buy(c, 'BTC', '1', '50000', '2025-01-05T10:00:00Z')
      sell(c, 'BTC', '0.5', '50000', '2026-05-15T10:00:00Z')
    end
    oversold = lambda do |c|
      kraken(c, :buy, 'BTC', '1', '50000', '2026-01-05T10:00:00Z')
      kraken(c, :sell, 'BTC', '1.5', '90000', '2026-02-05T10:00:00Z')
      kraken(c, :sell, 'ETH', '1', '3000', '2026-02-06T10:00:00Z')
    end
    lost = lambda do |c|
      price('BTC', '2026-01-06', '80000', 'EUR')
      kraken(c, :buy, 'BTC', '1', '10000', '2026-01-05T10:00:00Z')
      kraken(c, :buy, 'BTC', '1', '50000', '2026-01-05T11:00:00Z')
      row(c, :lost, 'BTC', '1', '2026-01-06T10:00:00Z', exchange: venue(c, :kraken))
      kraken(c, :sell, 'BTC', '1', '70000', '2026-01-07T10:00:00Z')
    end
    unlinked = lambda do |c|
      price('BTC', '2026-01-06', '80000', 'EUR')
      kraken(c, :buy, 'BTC', '1', '10000', '2026-01-05T10:00:00Z')
      row(c, :withdrawal, 'BTC', '0.4', '2026-01-06T10:00:00Z', exchange: venue(c, :kraken))
      kraken(c, :sell, 'BTC', '0.6', '48000', '2026-01-07T10:00:00Z')
    end
    # A coin fee that belongs to no trade (a Kraken fee row in BTC): a disposal at market value where spending a coin is
    # one, else its lots consumed at zero proceeds; named either way.
    coin_fee = lambda do |c|
      price('BTC', '2026-02-01', '80000', 'EUR')
      kraken(c, :buy, 'BTC', '1', '10000', '2025-01-05T10:00:00Z')
      kraken(c, :buy, 'BTC', '1', '70000', '2026-01-05T10:00:00Z')
      row(c, :fee, 'BTC', '0.01', '2026-02-01T10:00:00Z', exchange: venue(c, :kraken))
      kraken(c, :sell, 'BTC', '1', '90000', '2026-03-01T10:00:00Z')
    end
    each = lambda do |country, zone|
      cc = country.downcase
      [
        { name: "#{cc}_buy_sell_income_deposit_stated", country:, expect: 'parity', setup: ->(c) { user_zone(c, zone); eur.(c) } },
        { name: "#{cc}_prior_year_only", country:, year: 2025, expect: 'parity', setup: ->(c) { user_zone(c, zone); eur.(c) } },
        { name: "#{cc}_missing_price", country:, expect: 'parity', setup: ->(c) { user_zone(c, zone); missing.(c) } },
        { name: "#{cc}_swap_chain", country:, expect: country == 'SK' ? 'diverges: D-TAX-SK-SWAPS' : 'parity', setup: ->(c) { user_zone(c, zone); chain.(c) } },
        # Portugal: the USDT sold for euros on Kraken within 24 hours of the swap is the immediate fiat exit.
        # The USDT sold for euros on Kraken is valued at the euros received (and in Portugal is the immediate fiat exit).
        { name: "#{cc}_stablecoin_kept", country:, expect: 'diverges: D-TAX-STABLE-VALUE', setup: ->(c) { user_zone(c, zone); stable.(c) } },
        # Where swaps are not taxable, a stablecoin under the flag is cash: its sale for euros is no second disposal.
        # D-TAX-STABLECOIN-QUESTION: the flag is read only where the question is asked, and no served country asks it,
        # so Portugal computes its stablecoins as coins under the flag too.
        { name: "#{cc}_stablecoin_as_fiat", country:, stablecoin: true,
          expect: if %w[DE ES SK].include?(country) then 'diverges: D-TAX-STABLE-VALUE'
                  elsif country == 'PT' then 'diverges: D-TAX-STABLECOIN-QUESTION'
                  else 'diverges: D-TAX-STABLE-CASH' end, setup: ->(c) { user_zone(c, zone); stable.(c) } },
        { name: "#{cc}_ecb_gap", country:, fx_gap: %w[2026-05-15], expect: 'parity', setup: ->(c) { user_zone(c, zone); gap.(c) } },
        { name: "#{cc}_oversold", country:, expect: 'diverges: D-TAX-UNCOVERED', setup: ->(c) { user_zone(c, zone); oversold.(c) } },
        { name: "#{cc}_lost", country:, expect: 'diverges: D-TAX-LOST', setup: ->(c) { user_zone(c, zone); lost.(c) } },
        { name: "#{cc}_unlinked_withdrawal", country:, expect: 'diverges: D-TAX-UNLINKED-DISCLOSED', setup: ->(c) { user_zone(c, zone); unlinked.(c) } },
        { name: "#{cc}_coin_fee", country:, expect: 'diverges: D-TAX-COIN-FEE', setup: ->(c) { user_zone(c, zone); coin_fee.(c) } }
      ]
    end
    each.('DE', 'Berlin') + each.('AT', 'Vienna') + each.('ES', 'Madrid') + each.('PT', 'Lisbon') + each.('PL', 'Warsaw') +
      each.('SK', 'Bratislava') + each.('BG', 'Sofia') + each.('CZ', 'Prague') + each.('GR', 'Athens') + each.('DK', 'Copenhagen') + each.('IT', 'Rome') +
      each.('IE', 'Dublin') + holding_edges
  end

  def holding_edges
    [
      # More than a year, on the calendar in the user's zone; 29 February's anniversary is 28 February.
      { name: 'de_holding_edges', country: 'DE', expect: 'parity', setup: lambda { |c|
        user_zone(c, 'Berlin')
        kraken(c, :buy, 'BTC', '1', '20000', '2025-03-10T10:00:00Z', ref: 'b1')
        kraken(c, :buy, 'BTC', '1', '21000', '2025-03-11T10:00:00Z', ref: 'b2')
        kraken(c, :sell, 'BTC', '1', '50000', '2026-03-10T10:00:00Z', ref: 's1')
        kraken(c, :sell, 'BTC', '1', '50000', '2026-03-12T10:00:00Z', ref: 's2')
      } },
      { name: 'de_feb29', country: 'DE', year: 2025, expect: 'parity', setup: lambda { |c|
        kraken(c, :buy, 'BTC', '1', '40000', '2024-02-29T10:00:00Z', ref: 'b1')
        kraken(c, :buy, 'BTC', '1', '40000', '2024-03-01T10:00:00Z', ref: 'b2')
        kraken(c, :sell, 'BTC', '1', '90000', '2025-02-28T10:00:00Z', ref: 's1')
        kraken(c, :sell, 'BTC', '1', '90000', '2025-03-01T10:00:00Z', ref: 's2')
      } },
      # 23:30 UTC on 10 March is 11 March in Berlin: the anniversary is local, so the 11 March sale is not yet exempt.
      { name: 'de_user_zone_anniversary', country: 'DE', expect: 'parity', setup: lambda { |c|
        user_zone(c, 'Berlin')
        kraken(c, :buy, 'BTC', '1', '20000', '2025-03-10T23:30:00Z', ref: 'b1')
        kraken(c, :sell, 'BTC', '1', '50000', '2026-03-11T10:00:00Z', ref: 's1')
      } },
      # One sale over an exempt lot and a taxable one, the fee shared and every column settled in cents.
      { name: 'de_mixed_term_cents', country: 'DE', expect: 'parity', setup: lambda { |c|
        kraken(c, :buy, 'BTC', '0.333', '10000.01', '2024-06-01T10:00:00Z', ref: 'b1')
        kraken(c, :buy, 'BTC', '0.667', '30000.02', '2026-01-01T10:00:00Z', ref: 'b2')
        kraken(c, :sell, 'BTC', '1', '33333.33', '2026-03-01T10:00:00Z', fee: '10.01', ref: 's1')
      } },
      # Berlin's New Year's Eve after midnight is the next year; Los Angeles' before midnight is this one.
      { name: 'de_new_year_berlin', country: 'DE', year: 2025, expect: 'parity', setup: lambda { |c|
        user_zone(c, 'Berlin')
        kraken(c, :buy, 'BTC', '1', '20000', '2025-03-10T10:00:00Z', ref: 'b1')
        kraken(c, :sell, 'BTC', '0.5', '50000', '2025-12-31T23:30:00Z', ref: 's1')
        kraken(c, :sell, 'BTC', '0.5', '50000', '2025-12-31T22:30:00Z', ref: 's2')
      } },
      { name: 'us_new_year_los_angeles', year: 2025, expect: 'parity', setup: lambda { |c|
        user_zone(c, 'Pacific Time (US & Canada)')
        buy(c, 'BTC', '1', '20000', '2025-03-10T10:00:00Z')
        sell(c, 'BTC', '0.5', '50000', '2026-01-01T05:00:00Z')
        sell(c, 'BTC', '0.5', '50000', '2026-01-01T09:00:00Z')
      } },
      # Old stock: acquired before 1 March 2021 (in Vienna) and held more than a year.
      { name: 'at_old_stock', country: 'AT', expect: 'parity', setup: lambda { |c|
        user_zone(c, 'Vienna')
        kraken(c, :buy, 'BTC', '1', '40000', '2021-02-28T10:00:00Z', ref: 'b1')
        kraken(c, :buy, 'BTC', '1', '41000', '2021-02-28T23:30:00Z', ref: 'b2')
        kraken(c, :buy, 'BTC', '1', '42000', '2021-03-01T10:00:00Z', ref: 'b3')
        kraken(c, :sell, 'BTC', '3', '270000', '2026-03-01T10:00:00Z', ref: 's1')
      } },
      { name: 'at_old_stock_held_short', country: 'AT', year: 2021, expect: 'parity', setup: lambda { |c|
        kraken(c, :buy, 'BTC', '1', '40000', '2021-02-01T10:00:00Z', ref: 'b1')
        kraken(c, :sell, 'BTC', '1', '50000', '2021-12-01T10:00:00Z', ref: 's1')
      } },
      # An Austrian sweep of old and new stock into one coin keeps both ages.
      { name: 'at_sweep_keeps_ages', country: 'AT', expect: 'parity', setup: lambda { |c|
        price('ETH', '2025-06-01', '3000', 'EUR')
        price('BTC', '2025-06-01', '90000', 'EUR')
        kraken(c, :buy, 'ETH', '1', '1000', '2020-06-01T10:00:00Z', ref: 'b1')
        kraken(c, :buy, 'ETH', '1', '2000', '2024-06-01T10:00:00Z', ref: 'b2')
        price('ETH', '2025-05-01', '2900', 'EUR')
        price('BTC', '2026-02-01', '95000', 'EUR')
        transfer(c, 'ETH', '2', '2025-05-01T10:00:00Z', from: venue(c, :kraken), to: venue(c, :binance))
        swap(c, 'ETH', '2', 'BTC', '0.0666', '2025-06-01T10:00:00Z', ref: 'cv1')
        transfer(c, 'BTC', '0.0666', '2026-02-01T10:00:00Z', from: venue(c, :binance), to: venue(c, :kraken))
        kraken(c, :sell, 'BTC', '0.0666', '6600', '2026-03-01T10:00:00Z', ref: 's1')
      } },
      # 365 days, counted inclusive.
      { name: 'pt_365_days', country: 'PT', expect: 'parity', setup: lambda { |c|
        user_zone(c, 'Lisbon')
        kraken(c, :buy, 'BTC', '1', '20000', '2025-03-10T10:00:00Z', ref: 'b1')
        kraken(c, :buy, 'BTC', '1', '20000', '2025-03-11T10:00:00Z', ref: 'b2')
        kraken(c, :sell, 'BTC', '1', '50000', '2026-03-10T09:00:00Z', ref: 's1')
        kraken(c, :sell, 'BTC', '1', '50000', '2026-03-10T11:00:00Z', ref: 's2')
      } },
      { name: 'pt_feb29', country: 'PT', year: 2025, expect: 'parity', setup: lambda { |c|
        kraken(c, :buy, 'BTC', '1', '40000', '2024-02-29T10:00:00Z', ref: 'b1')
        kraken(c, :buy, 'BTC', '1', '40000', '2024-03-01T10:00:00Z', ref: 'b2')
        kraken(c, :sell, 'BTC', '1', '90000', '2025-02-28T10:00:00Z', ref: 's1')
        kraken(c, :sell, 'BTC', '1', '90000', '2025-02-28T11:00:00Z', ref: 's2')
      } },
      # A swap restarts the Portuguese holding period; a long-held coin swapped into a stablecoin and sold for euros
      # within 24 hours keeps it, later than that it does not.
      { name: 'pt_swap_resets', country: 'PT', expect: 'parity', setup: lambda { |c|
        price('ETH', '2025-06-01', '3000', 'EUR')
        price('BTC', '2025-06-01', '90000', 'EUR')
        kraken(c, :buy, 'ETH', '3', '3000', '2023-01-01T10:00:00Z', ref: 'b1')
        swap(c, 'ETH', '3', 'BTC', '0.1', '2025-06-01T10:00:00Z', ref: 'cv1')
        kraken(c, :sell, 'BTC', '0.1', '9500', '2026-03-01T10:00:00Z', ref: 's1')
      } },
      { name: 'pt_immediate_fiat_exit_quoted', country: 'PT', expect: 'parity', setup: lambda { |c|
        price('BTC', '2026-02-01', '86000', 'EUR')
        price('USDT', '2026-02-01', '0.86', 'EUR')
        kraken(c, :buy, 'BTC', '1', '20000', '2024-01-01T10:00:00Z', ref: 'b1')
        swap(c, 'BTC', '0.5', 'USDT', '50000', '2026-02-01T23:00:00Z', ref: 'cv1')
        row(c, :sell, 'USDT', '25000', '2026-02-02T22:00:00Z', exchange: venue(c, :binance), quote_currency: 'EUR', quote_amount: '21500', tx_id: 'q1')
        row(c, :sell, 'USDT', '25000', '2026-02-03T00:00:00Z', exchange: venue(c, :binance), quote_currency: 'EUR', quote_amount: '21500', tx_id: 'q2')
      } },
      # The same exit on Kraken: the USDT row has no quote of its own, its euros are the group's cash row.
      { name: 'pt_immediate_fiat_exit_kraken', country: 'PT', expect: 'diverges: D-TAX-STABLE-VALUE', setup: lambda { |c|
        price('BTC', '2026-02-01', '86000', 'EUR')
        price('USDT', '2026-02-01', '0.86', 'EUR')
        kraken(c, :buy, 'BTC', '1', '20000', '2024-01-01T10:00:00Z', ref: 'b1')
        swap(c, 'BTC', '0.5', 'USDT', '50000', '2026-02-01T23:00:00Z', ref: 'cv1')
        kraken(c, :sell, 'USDT', '25000', '21500', '2026-02-02T22:00:00Z', ref: 's1')
        kraken(c, :sell, 'USDT', '25000', '21500', '2026-02-03T00:00:00Z', ref: 's2')
      } },
      # Portugal: swaps are not taxable (the named divergence of each); Slovakia: each swap is a sale (D-TAX-SK-SWAPS).
      *%w[PT SK].flat_map do |cc|
        label = ->(d) { cc == 'SK' ? 'diverges: D-TAX-SK-SWAPS' : "diverges: #{d}" }
        [
        # A swap-in carrying two tranches of equal worth, with a fee in BNB: the whole fee joins the new coin's cost.
        { name: "#{cc.downcase}_swap_fee_two_lots", country: cc, expect: label.('D-TAX-SWAP-FEE'), setup: lambda { |c|
          price('ETH', '2026-02-01', '3000', 'EUR')
          price('BTC', '2026-02-01', '90000', 'EUR')
          price('BNB', '2026-02-01', '500', 'EUR')
          kraken(c, :buy, 'BNB', '1', '400', '2025-01-02T10:00:00Z', ref: 'b0')
          kraken(c, :buy, 'ETH', '1', '1000', '2025-01-03T10:00:00Z', ref: 'b1')
          kraken(c, :buy, 'ETH', '1', '2000', '2025-01-04T10:00:00Z', ref: 'b2')
          row(c, :swap_out, 'ETH', '2', '2026-02-01T10:00:00Z', exchange: venue(c, :binance), group_id: 'sf', tx_id: 'sf-o')
          row(c, :swap_in, 'BTC', '0.0666', '2026-02-01T10:00:00Z', exchange: venue(c, :binance), group_id: 'sf', tx_id: 'sf-i', fee_currency: 'BNB', fee_amount: '0.02')
          row(c, :sell, 'BTC', '0.0666', '2026-03-01T10:00:00Z', exchange: venue(c, :binance), quote_currency: 'EUR', quote_amount: '6500')
        } },
        # Cash and an unpriced coin of no basis bought BTC, with a fee in BNB: the fee joins the cash lot's cost.
        { name: "#{cc.downcase}_swap_fee_cash_only", country: cc, expect: label.('D-TAX-SWAP-FEE'), setup: lambda { |c|
          price('BTC', '2026-02-01', '90000', 'EUR')
          price('BNB', '2026-02-01', '500', 'EUR')
          kraken(c, :buy, 'BNB', '1', '400', '2025-01-02T10:00:00Z', ref: 'b0')
          row(c, :swap_out, 'NOPRICE', '5', '2026-02-01T10:00:00Z', exchange: venue(c, :kraken), group_id: 'cf', tx_id: 'cf-o')
          row(c, :sell, 'EUR', '100', '2026-02-01T10:00:00Z', exchange: venue(c, :kraken), group_id: 'cf', tx_id: 'cf-e')
          row(c, :swap_in, 'BTC', '0.002', '2026-02-01T10:00:00Z', exchange: venue(c, :kraken), group_id: 'cf', tx_id: 'cf-i', fee_currency: 'BNB', fee_amount: '0.02')
          kraken(c, :sell, 'BTC', '0.002', '200', '2026-03-01T10:00:00Z', ref: 's1')
        } },
        # Swap legs with nothing going the other way.
        { name: "#{cc.downcase}_orphan_swaps", country: cc, expect: label.('D-TAX-ORPHAN-SWAP'), setup: lambda { |c|
          price('ETH', '2026-02-01', '3000', 'EUR')
          price('BTC', '2026-02-03', '90000', 'EUR')
          kraken(c, :buy, 'ETH', '1', '1000', '2025-01-03T10:00:00Z', ref: 'b1')
          row(c, :swap_out, 'ETH', '1', '2026-02-01T10:00:00Z', exchange: venue(c, :binance), group_id: 'o1', tx_id: 'o1-o')
          row(c, :swap_in, 'BTC', '0.03', '2026-02-03T10:00:00Z', exchange: venue(c, :binance), group_id: 'o2', tx_id: 'o2-i')
          row(c, :sell, 'BTC', '0.03', '2026-03-01T10:00:00Z', exchange: venue(c, :binance), quote_currency: 'EUR', quote_amount: '2900')
        } }
        ]
      end,
      # Staking received at 00:30 on 1 January in Berlin is that year's income.
      { name: 'de_income_new_year', country: 'DE', expect: 'diverges: D-TAX-LOCAL-YEAR', setup: lambda { |c|
        user_zone(c, 'Berlin')
        price('ETH', '2025-12-31', '3000', 'EUR')
        row(c, :staking_reward, 'ETH', '0.1', '2025-12-31T23:30:00Z', exchange: venue(c, :kraken))
      } },
      # The same trade where swaps are not taxable: the BTC's basis travels into the ETH, no sale until the ETH is sold.
      { name: 'pt_coin_quoted_pair', country: 'PT', expect: 'diverges: D-TAX-COIN-QUOTE', setup: lambda { |c|
        by = Exchanges::Bybit.create!(name: 'Bybit', maker_fee: '0.1', taker_fee: '0.1')
        price('BTC', '2026-02-01', '41000', 'EUR')
        price('ETH', '2026-02-01', '2560', 'EUR')
        kraken(c, :buy, 'BTC', '1', '25000', '2025-06-05T10:00:00Z', ref: 'b1')
        transfer(c, 'BTC', '1', '2025-07-01T10:00:00Z', from: venue(c, :kraken), to: by)
        row(c, :sell, 'BTC', '0.5', '2026-02-01T10:00:00Z', exchange: by, quote_currency: 'ETH', quote_amount: '8')
        row(c, :sell, 'ETH', '8', '2026-03-01T10:00:00Z', exchange: by, quote_currency: 'EUR', quote_amount: '22000')
      } },
      # Where swaps are not taxable, a swap's fee in the coin given up leaves the lots: the later sale takes none of it.
      { name: 'pt_swap_out_fee_in_coin', country: 'PT', expect: 'diverges: D-TAX-COIN-FEE', setup: lambda { |c|
        by = Exchanges::Bybit.create!(name: 'Bybit', maker_fee: '0.1', taker_fee: '0.1')
        price('BTC', '2026-02-01', '41000', 'EUR')
        price('ETH', '2026-02-01', '2560', 'EUR')
        row(c, :buy, 'BTC', '1', '2026-01-05T10:00:00Z', exchange: by, quote_currency: 'EUR', quote_amount: '10000')
        row(c, :buy, 'BTC', '1', '2026-01-06T10:00:00Z', exchange: by, quote_currency: 'EUR', quote_amount: '50000')
        row(c, :sell, 'BTC', '0.5', '2026-02-01T10:00:00Z', exchange: by, quote_currency: 'ETH', quote_amount: '8', fee_currency: 'BTC', fee_amount: '0.1')
        row(c, :sell, 'BTC', '1.4', '2026-03-01T10:00:00Z', exchange: by, quote_currency: 'EUR', quote_amount: '70000')
      } },
      # D-TAX-SWAP-FEE-EXHAUSTED: 0.002 BTC (cost €100) for 0.05 ETH quoted in BTC, the fee 0.05 ETH: nothing received, no
      # lot, no loss; the €100 is listed. Austria: the same with an empty ETH pool; with ETH held (€200), it joins that pool
      # (Rz 6178ad) and nothing is listed: the later sale of that ETH (1 for €500) has cost €300. A swap in 2025 is not in
      # the 2026 report's list.
      *[['pt', 'PT', 'UTC'], ['at', 'AT', 'Vienna']].map do |cc, country, zone|
        { name: "#{cc}_swap_fee_exhausted", country:, expect: 'diverges: D-TAX-SWAP-FEE-EXHAUSTED', setup: lambda { |c|
          user_zone(c, zone)
          by = Exchanges::Bybit.create!(name: 'Bybit', maker_fee: '0.1', taker_fee: '0.1')
          price('BTC', '2026-02-01', '50000', 'EUR')
          price('ETH', '2026-02-01', '2000', 'EUR')
          row(c, :buy, 'BTC', '0.002', '2026-01-05T10:00:00Z', exchange: by, quote_currency: 'EUR', quote_amount: '100')
          row(c, :buy, 'ETH', '0.05', '2026-02-01T10:00:00Z', exchange: by, quote_currency: 'BTC', quote_amount: '0.002', fee_currency: 'ETH', fee_amount: '0.05')
        } }
      end,
      { name: 'at_swap_fee_exhausted_into_pool', country: 'AT', expect: 'diverges: D-TAX-SWAP-FEE-EXHAUSTED', setup: lambda { |c|
        user_zone(c, 'Vienna')
        by = Exchanges::Bybit.create!(name: 'Bybit', maker_fee: '0.1', taker_fee: '0.1')
        price('BTC', '2026-02-01', '50000', 'EUR')
        price('ETH', '2026-02-01', '2000', 'EUR')
        row(c, :buy, 'ETH', '1', '2025-06-02T10:00:00Z', exchange: by, quote_currency: 'EUR', quote_amount: '200')
        row(c, :buy, 'BTC', '0.002', '2026-01-05T10:00:00Z', exchange: by, quote_currency: 'EUR', quote_amount: '100')
        row(c, :buy, 'ETH', '0.05', '2026-02-01T10:00:00Z', exchange: by, quote_currency: 'BTC', quote_amount: '0.002', fee_currency: 'ETH', fee_amount: '0.05')
        row(c, :sell, 'ETH', '1', '2026-03-02T10:00:00Z', exchange: by, quote_currency: 'EUR', quote_amount: '500')
      } },
      { name: 'pt_swap_fee_exhausted_last_year', country: 'PT', expect: 'parity', setup: lambda { |c|
        by = Exchanges::Bybit.create!(name: 'Bybit', maker_fee: '0.1', taker_fee: '0.1')
        price('BTC', '2025-02-03', '50000', 'EUR')
        price('ETH', '2025-02-03', '2000', 'EUR')
        row(c, :buy, 'BTC', '0.002', '2025-01-06T10:00:00Z', exchange: by, quote_currency: 'EUR', quote_amount: '100')
        row(c, :buy, 'ETH', '0.05', '2025-02-03T10:00:00Z', exchange: by, quote_currency: 'BTC', quote_amount: '0.002', fee_currency: 'ETH', fee_amount: '0.05')
        row(c, :buy, 'SOL', '1', '2026-02-02T10:00:00Z', exchange: by, quote_currency: 'EUR', quote_amount: '100')
        row(c, :sell, 'SOL', '1', '2026-03-02T10:00:00Z', exchange: by, quote_currency: 'EUR', quote_amount: '150')
      } },
      # Slovakia: a coin exchanged for a coin is a sale at the market value of what came in, which becomes its cost.
      { name: 'sk_taxable_swap', country: 'SK', expect: 'diverges: D-TAX-SK-SWAPS', setup: lambda { |c|
        price('ETH', '2026-02-01', '3000', 'EUR')
        price('BTC', '2026-02-01', '100000', 'EUR')
        kraken(c, :buy, 'ETH', '1', '1000', '2025-06-01T10:00:00Z', ref: 'b1')
        swap(c, 'ETH', '1', 'BTC', '0.03', '2026-02-01T10:00:00Z', ref: 'cv1')
        kraken(c, :sell, 'BTC', '0.03', '3500', '2026-03-01T10:00:00Z', ref: 's1')
      } },
      # Per wallet, a same-second withdrawal carries its lots before a sale on the far venue, a linked deposit between them.
      { name: 'de_same_second_transfer_past_deposit', country: 'DE', expect: 'parity', setup: lambda { |c|
        by = Exchanges::Bybit.create!(name: 'Bybit', maker_fee: '0.1', taker_fee: '0.1')
        kr = venue(c, :kraken)
        bn = venue(c, :binance)
        row(c, :buy, 'BTC', '1', '2026-01-05T10:00:00Z', exchange: by, quote_currency: 'EUR', quote_amount: '10000')
        row(c, :sell, 'BTC', '1', '2026-02-05T10:00:00Z', exchange: bn, quote_currency: 'EUR', quote_amount: '20000')
        row(c, :deposit, 'BTC', '1', '2026-02-05T10:00:00Z', exchange: kr)
        row(c, :withdrawal, 'BTC', '1', '2026-02-05T09:00:00Z', exchange: by, linked_transaction_id: AccountTransaction.order(:id).last.id)
        row(c, :deposit, 'BTC', '1', '2026-02-05T10:10:00Z', exchange: bn)
        row(c, :withdrawal, 'BTC', '1', '2026-02-05T10:00:00Z', exchange: kr, linked_transaction_id: AccountTransaction.order(:id).last.id)
      } },
      # Per wallet, a chain of same-second withdrawals (Bybit to Kraken to Binance), booked in that order, walks in the order
      # its funds allow before the sale on Binance; Rails' order reversed them (D-TRACKER-TRANSFER-ORDER; its pool takes the same lot).
      { name: 'de_same_second_chained_transfer', country: 'DE', expect: 'parity', setup: lambda { |c|
        by = Exchanges::Bybit.create!(name: 'Bybit', maker_fee: '0.1', taker_fee: '0.1')
        kr = venue(c, :kraken)
        bn = venue(c, :binance)
        row(c, :buy, 'BTC', '1', '2026-01-05T10:00:00Z', exchange: by, quote_currency: 'EUR', quote_amount: '10000')
        row(c, :sell, 'BTC', '1', '2026-02-05T10:00:00Z', exchange: bn, quote_currency: 'EUR', quote_amount: '20000')
        transfer(c, 'BTC', '1', '2026-02-05T10:00:00Z', from: by, to: kr)
        transfer(c, 'BTC', '1', '2026-02-05T10:00:00Z', from: kr, to: bn)
      } },
      # US, per account from 2025: a same-second round trip Binance to Kraken and back, booked out-first, takes the lot out
      # and home before the sale (Rails' order reversed them and sent Kraken's coins first, of which it held none).
      { name: 'us_reciprocal_transfer_2025', expect: 'parity', setup: lambda { |c|
        bn = venue(c, :binance)
        kr = venue(c, :kraken)
        buy(c, 'BTC', '1', '30000', '2026-01-05T10:00:00Z', exchange: bn)
        transfer(c, 'BTC', '1', '2026-02-05T10:00:00Z', from: bn, to: kr)
        transfer(c, 'BTC', '1', '2026-02-05T10:00:00Z', from: kr, to: bn)
        sell(c, 'BTC', '1', '40000', '2026-03-01T10:00:00Z', exchange: bn)
      } },
      # An unlinked withdrawal's own fee in the coin withdrawn leaves the lots as a disposal: 0.0005 BTC, €40 for €15.
      { name: 'de_unlinked_withdrawal_fee', country: 'DE', expect: 'diverges: D-TAX-COIN-FEE', setup: lambda { |c|
        price('BTC', '2026-02-01', '80000', 'EUR')
        kraken(c, :buy, 'BTC', '1', '30000', '2025-01-05T10:00:00Z', ref: 'b1')
        row(c, :withdrawal, 'BTC', '0.1', '2026-02-01T10:00:00Z', exchange: venue(c, :kraken), fee_currency: 'BTC', fee_amount: '0.0005')
        # The fee is inside the 0.1: Kraken keeps 0.9, all of it covered (cost 27,000).
        kraken(c, :sell, 'BTC', '0.9', '72000', '2026-03-01T10:00:00Z', ref: 's1')
      } },
      # Portugal: BTC for USDT in Kraken's two grouped rows is a swap, not a sale: the USDT carries the BTC's €20,000.
      { name: 'pt_kraken_coin_for_stablecoin', country: 'PT', expect: 'diverges: D-TAX-COIN-QUOTE', setup: lambda { |c|
        price('BTC', '2025-06-10', '55000', 'EUR')
        price('USDT', '2025-06-10', '0.9', 'EUR')
        kraken(c, :buy, 'BTC', '1', '20000', '2024-06-01T10:00:00Z', ref: 'b1')
        kraken(c, :sell, 'BTC', '1', '60000', '2025-06-10T10:00:00Z', fiat: 'USDT', ref: 's1')
        kraken(c, :sell, 'USDT', '60000', '54000', '2026-03-01T10:00:00Z', ref: 's2')
      } },
      # Portugal: USDT for USDC in Kraken's grouped rows is crypto for crypto too: the USDC carries the USDT's €800.
      { name: 'pt_kraken_stablecoin_for_stablecoin', country: 'PT', expect: 'diverges: D-TAX-COIN-QUOTE', setup: lambda { |c|
        kraken(c, :buy, 'USDT', '1000', '800', '2024-06-01T10:00:00Z', ref: 'b1')
        ex = venue(c, :kraken)
        row(c, :sell, 'USDT', '1000', '2025-06-10T10:00:00Z', exchange: ex, group_id: 'ss', tx_id: 'ss-USDT')
        row(c, :buy, 'USDC', '1000', '2025-06-10T10:00:00Z', exchange: ex, group_id: 'ss', tx_id: 'ss-USDC')
        kraken(c, :sell, 'USDC', '1000', '1100', '2026-03-01T10:00:00Z', ref: 's2')
      } },
      # Germany: the order of use is kept per wallet; a sale on Kraken takes Kraken's lot, not Bybit's older one.
      { name: 'de_per_wallet', country: 'DE', expect: 'diverges: D-TAX-DE-WALLET', setup: lambda { |c|
        by = Exchanges::Bybit.create!(name: 'Bybit', maker_fee: '0.1', taker_fee: '0.1')
        row(c, :buy, 'BTC', '1', '2024-01-05T10:00:00Z', exchange: by, quote_currency: 'EUR', quote_amount: '10000')
        kraken(c, :buy, 'BTC', '1', '50000', '2026-01-05T10:00:00Z', ref: 'b1')
        kraken(c, :sell, 'BTC', '1', '60000', '2026-02-05T10:00:00Z', ref: 's1')
      } },
      { name: 'sk_no_holding_period', country: 'SK', expect: 'parity', setup: lambda { |c|
        kraken(c, :buy, 'BTC', '1', '20000', '2020-03-10T10:00:00Z', ref: 'b1')
        kraken(c, :sell, 'BTC', '1', '50000', '2026-03-10T10:00:00Z', ref: 's1')
      } },
      { name: 'es_taxable_swap', country: 'ES', expect: 'parity', setup: lambda { |c|
        price('ETH', '2026-02-01', '3000', 'EUR')
        price('BTC', '2026-02-01', '90000', 'EUR')
        kraken(c, :buy, 'ETH', '3', '6000', '2025-06-01T10:00:00Z', ref: 'b1')
        swap(c, 'ETH', '3', 'BTC', '0.1', '2026-02-01T10:00:00Z', ref: 'cv1')
        kraken(c, :sell, 'BTC', '0.1', '9500', '2026-03-01T10:00:00Z', ref: 's1')
      } },
      { name: 'pl_usd_in_zloty', country: 'PL', expect: 'parity', setup: lambda { |c|
        user_zone(c, 'Warsaw')
        buy(c, 'BTC', '1', '50000', '2025-01-05T10:00:00Z')
        sell(c, 'BTC', '0.5', '50000', '2026-03-05T10:00:00Z')
      } }
    ]
  end

  # Rails' own test/models/tax cases, as rows the job reads.
  def rails_cases
    k = ->(c) { venue(c, :kraken) }
    b = ->(c) { venue(c, :binance) }
    [
      { name: 'de_fiat_acquisition_fee', country: 'DE', year: 2024, expect: 'parity', setup: lambda { |c|
        row(c, :buy, 'BTC', '1', '2024-01-01T00:00:00Z', exchange: b.(c), quote_currency: 'EUR', quote_amount: '10000', fee_currency: 'EUR', fee_amount: '50')
        row(c, :sell, 'BTC', '1', '2024-06-01T00:00:00Z', exchange: b.(c), quote_currency: 'EUR', quote_amount: '30000')
      } },
      { name: 'de_kraken_fee_on_sale', country: 'DE', year: 2024, expect: 'parity', setup: lambda { |c|
        price('BTC', '2024-01-10', '20000', 'EUR')
        kraken(c, :buy, 'BTC', '0.5', '10000', '2024-01-10T00:00:00Z', ref: 'R1')
        row(c, :sell, 'BTC', '0.5', '2024-06-01T00:00:00Z', exchange: k.(c), quote_currency: 'EUR', quote_amount: '30000', group_id: 'R2', tx_id: 'R2-BTC')
        row(c, :buy, 'EUR', '30000', '2024-06-01T00:00:00Z', exchange: k.(c), group_id: 'R2', tx_id: 'R2-EUR', fee_currency: 'EUR', fee_amount: '78')
      } },
      { name: 'de_kraken_aud_settlement', country: 'DE', year: 2024, expect: 'parity', setup: lambda { |c|
        price('BTC', '2024-01-10', '20000', 'EUR')
        kraken(c, :buy, 'BTC', '0.5', '16000', '2024-01-10T00:00:00Z', fiat: 'AUD', ref: 'R1')
        row(c, :sell, 'BTC', '0.5', '2024-06-01T00:00:00Z', exchange: k.(c), quote_currency: 'EUR', quote_amount: '30000')
      } },
      { name: 'de_stored_zero_price', country: 'DE', year: 2024, expect: 'parity', setup: lambda { |c|
        price('BTC', '2024-01-10', '0', 'EUR')
        row(c, :deposit, 'BTC', '1', '2024-01-10T00:00:00Z', exchange: b.(c))
        row(c, :sell, 'BTC', '1', '2024-06-01T00:00:00Z', exchange: b.(c), quote_currency: 'EUR', quote_amount: '30000')
      } },
      # Cold storage and back, linked, with a network fee: the basis travels and the fee's units leave at zero.
      { name: 'de_linked_transfer_keeps_basis', country: 'DE', expect: 'parity', setup: lambda { |c|
        kraken(c, :buy, 'BTC', '1', '20000', '2025-06-01T10:00:00Z', ref: 'b1')
        kraken(c, :buy, 'BTC', '1', '30000', '2025-07-01T10:00:00Z', ref: 'b2')
        transfer(c, 'BTC', '1.001', '2025-08-01T10:00:00Z', from: k.(c), to: b.(c), received: '1')
        row(c, :sell, 'BTC', '1', '2026-03-01T10:00:00Z', exchange: b.(c), quote_currency: 'EUR', quote_amount: '80000')
      } },
      { name: 'de_fiat_deposit_not_named', country: 'DE', expect: 'parity', setup: lambda { |c|
        row(c, :deposit, 'EUR', '5000', '2026-01-02T10:00:00Z', exchange: k.(c))
        kraken(c, :buy, 'BTC', '0.1', '5000', '2026-01-05T10:00:00Z')
      } },
      { name: 'de_income_types_and_totals', country: 'DE', expect: 'parity', setup: lambda { |c|
        price('ETH', '2026-01-01', '3000', 'EUR')
        price('ETH', '2026-02-01', '3100', 'EUR')
        price('SOL', '2026-03-01', '150', 'EUR')
        price('BTC', '2026-04-01', '80000', 'EUR')
        row(c, :staking_reward, 'ETH', '0.01', '2026-01-01T00:00:00Z', exchange: k.(c))
        row(c, :staking_reward, 'ETH', '0.02', '2026-02-01T00:00:00Z', exchange: k.(c))
        row(c, :lending_interest, 'SOL', '1', '2026-03-01T00:00:00Z', exchange: b.(c))
        row(c, :mining, 'BTC', '0.001', '2026-04-01T00:00:00Z', exchange: b.(c))
        row(c, :other_income, 'USD', '12.5', '2026-04-02T00:00:00Z', exchange: k.(c))
        row(c, :other_income, 'EUR', '7', '2026-04-03T00:00:00Z', exchange: k.(c))
        row(c, :staking_reward, 'ETH', '0.5', '2025-12-31T23:00:00Z', exchange: k.(c))
      } },
      { name: 'de_unpriceable_fiat_income', country: 'DE', expect: 'parity', setup: lambda { |c|
        row(c, :other_income, 'AUD', '20', '2026-04-02T00:00:00Z', exchange: k.(c))
      } },
      { name: 'us_return_of_capital_refuses', expect: 'parity', setup: lambda { |c|
        buy(c, 'BTC', '1', '100', '2026-01-05T10:00:00Z')
        row(c, :return_of_capital, 'BTC', '1', '2026-02-05T10:00:00Z', quote_currency: 'USD', quote_amount: '150', raw_data: { 'per_share_amount' => '150' })
      } },
      { name: 'us_return_of_capital_other_year', expect: 'parity', setup: lambda { |c|
        buy(c, 'BTC', '1', '100', '2025-01-05T10:00:00Z')
        row(c, :return_of_capital, 'BTC', '1', '2025-02-05T10:00:00Z', quote_currency: 'USD', quote_amount: '150', raw_data: { 'per_share_amount' => '150' })
        sell(c, 'BTC', '1', '500', '2026-02-05T10:00:00Z')
      } },
      { name: 'us_return_of_capital_by_dollars', expect: 'parity', setup: lambda { |c|
        buy(c, 'BTC', '1', '100', '2025-01-05T10:00:00Z')
        buy(c, 'BTC', '1', '300', '2025-01-06T10:00:00Z')
        row(c, :return_of_capital, 'BTC', '2', '2025-02-05T10:00:00Z', quote_currency: 'USD', quote_amount: '150')
        sell(c, 'BTC', '2', '1000', '2026-02-05T10:00:00Z')
      } },
      { name: 'us_split_and_withholding', expect: 'parity', setup: lambda { |c|
        buy(c, 'BTC', '1', '100', '2025-01-05T10:00:00Z')
        buy(c, 'BTC', '1', '300', '2025-03-05T10:00:00Z')
        row(c, :adjustment, 'BTC', '2', '2025-06-05T10:00:00Z')
        row(c, :withholding_tax, 'BTC', '0.1', '2025-06-06T10:00:00Z')
        sell(c, 'BTC', '3', '900', '2026-02-05T10:00:00Z')
      } },
      { name: 'us_adjustment_clears_and_opens', expect: 'parity', setup: lambda { |c|
        price('ETH', '2026-01-10', '3000')
        buy(c, 'BTC', '1', '100', '2025-01-05T10:00:00Z')
        row(c, :adjustment, 'BTC', '-1', '2025-06-05T10:00:00Z')
        row(c, :adjustment, 'ETH', '2', '2026-01-10T10:00:00Z')
        sell(c, 'ETH', '1', '3500', '2026-02-05T10:00:00Z')
      } },
      { name: 'de_fees_in_base_third_and_quote', country: 'DE', expect: 'diverges: D-TAX-COIN-FEE', setup: lambda { |c|
        price('BNB', '2026-01-05', '500', 'EUR')
        price('BNB', '2026-03-05', '550', 'EUR')
        price('USDT', '2026-01-06', '0.86', 'EUR')
        row(c, :buy, 'BNB', '1', '2026-01-04T10:00:00Z', exchange: b.(c), quote_currency: 'EUR', quote_amount: '480')
        row(c, :buy, 'USDT', '6006', '2026-01-04T11:00:00Z', exchange: b.(c), quote_currency: 'EUR', quote_amount: '5165')
        row(c, :buy, 'BTC', '1', '2026-01-05T10:00:00Z', exchange: b.(c), quote_currency: 'EUR', quote_amount: '50000', fee_currency: 'BNB', fee_amount: '0.1')
        row(c, :buy, 'ETH', '2', '2026-01-06T10:00:00Z', exchange: b.(c), quote_currency: 'USDT', quote_amount: '6000', fee_currency: 'USDT', fee_amount: '6')
        row(c, :buy, 'SOL', '10', '2026-01-07T10:00:00Z', exchange: b.(c), quote_currency: 'EUR', quote_amount: '1500', fee_currency: 'SOL', fee_amount: '0.01')
        row(c, :sell, 'BTC', '0.5', '2026-03-05T10:00:00Z', exchange: b.(c), quote_currency: 'EUR', quote_amount: '40000', fee_currency: 'BNB', fee_amount: '0.05')
        row(c, :sell, 'BNB', '0.85', '2026-03-06T10:00:00Z', exchange: b.(c), quote_currency: 'EUR', quote_amount: '470')
        row(c, :sell, 'SOL', '9.99', '2026-03-07T10:00:00Z', exchange: b.(c), quote_currency: 'EUR', quote_amount: '1600')
      } },
      { name: 'de_formula_symbol', country: 'DE', expect: 'parity', setup: lambda { |c|
        row(c, :buy, '=HACK', '1', '2026-01-05T10:00:00Z', exchange: b.(c), quote_currency: 'EUR', quote_amount: '10')
        row(c, :sell, '=HACK', '1', '2026-02-05T10:00:00Z', exchange: b.(c), quote_currency: 'EUR', quote_amount: '20', tx_id: '@cmd')
      } },
      # A dust sweep of several coins into one: every leg's basis carried.
      { name: 'at_sweep_several_coins', country: 'AT', expect: 'parity', setup: lambda { |c|
        price('ETH', '2026-02-01', '3000', 'EUR')
        price('SOL', '2026-02-01', '150', 'EUR')
        price('BNB', '2026-02-01', '500', 'EUR')
        kraken(c, :buy, 'ETH', '0.1', '200', '2025-01-01T10:00:00Z', ref: 'b1')
        kraken(c, :buy, 'SOL', '2', '100', '2025-02-01T10:00:00Z', ref: 'b2')
        price('ETH', '2026-01-15', '3000', 'EUR')
        price('SOL', '2026-01-15', '150', 'EUR')
        transfer(c, 'ETH', '0.1', '2026-01-15T10:00:00Z', from: k.(c), to: b.(c))
        transfer(c, 'SOL', '2', '2026-01-15T11:00:00Z', from: k.(c), to: b.(c))
        row(c, :swap_out, 'ETH', '0.1', '2026-02-01T10:00:00Z', exchange: b.(c), group_id: 'dust', tx_id: 'd1')
        row(c, :swap_out, 'SOL', '2', '2026-02-01T10:00:00Z', exchange: b.(c), group_id: 'dust', tx_id: 'd2')
        row(c, :swap_in, 'BNB', '1.2', '2026-02-01T10:00:00Z', exchange: b.(c), group_id: 'dust', tx_id: 'd3')
        row(c, :sell, 'BNB', '1.2', '2026-03-01T10:00:00Z', exchange: b.(c), quote_currency: 'EUR', quote_amount: '700')
      } },
      # One swap credited in two fills of one coin (by amount), and a swap into two coins (by value).
      { name: 'at_swap_fills_and_values', country: 'AT', expect: 'parity', setup: lambda { |c|
        price('ETH', '2026-02-02', '3000', 'EUR')
        price('BTC', '2026-02-02', '90000', 'EUR')
        price('SOL', '2026-02-02', '150', 'EUR')
        price('ETH', '2026-02-01', '3000', 'EUR')
        price('BTC', '2026-02-01', '90000', 'EUR')
        price('SOL', '2026-02-01', '150', 'EUR')
        kraken(c, :buy, 'ETH', '2', '2000', '2025-01-01T10:00:00Z', ref: 'b1')
        kraken(c, :buy, 'ETH', '2', '4000', '2025-06-01T10:00:00Z', ref: 'b2')
        price('ETH', '2026-01-15', '3000', 'EUR')
        transfer(c, 'ETH', '4', '2026-01-15T10:00:00Z', from: k.(c), to: b.(c))
        row(c, :swap_out, 'ETH', '1', '2026-02-01T10:00:00Z', exchange: b.(c), group_id: 'f1', tx_id: 'f1o')
        row(c, :swap_in, 'BTC', '0.01', '2026-02-01T10:00:00Z', exchange: b.(c), group_id: 'f1', tx_id: 'f1a')
        row(c, :swap_in, 'BTC', '0.0233', '2026-02-01T10:00:00Z', exchange: b.(c), group_id: 'f1', tx_id: 'f1b')
        row(c, :swap_out, 'ETH', '2', '2026-02-02T10:00:00Z', exchange: b.(c), group_id: 'f2', tx_id: 'f2o')
        row(c, :swap_in, 'BTC', '0.03', '2026-02-02T10:00:00Z', exchange: b.(c), group_id: 'f2', tx_id: 'f2a')
        row(c, :swap_in, 'SOL', '20', '2026-02-02T10:00:00Z', exchange: b.(c), group_id: 'f2', tx_id: 'f2b')
        row(c, :sell, 'BTC', '0.0633', '2026-03-01T10:00:00Z', exchange: b.(c), quote_currency: 'EUR', quote_amount: '6000')
        row(c, :sell, 'SOL', '20', '2026-03-02T10:00:00Z', exchange: b.(c), quote_currency: 'EUR', quote_amount: '3200')
      } },
      # A share nobody can weigh (one in-leg unpriced) is split evenly and marked assumed; the same group id on another
      # venue is another group; an out-leg the lots do not cover chains an assumed basis.
      { name: 'at_unweighable_and_uncovered_legs', country: 'AT', expect: 'parity', setup: lambda { |c|
        price('ETH', '2026-02-01', '3000', 'EUR')
        price('BTC', '2026-02-01', '90000', 'EUR')
        kraken(c, :buy, 'ETH', '1', '1000', '2025-01-01T10:00:00Z', ref: 'b1')
        price('ETH', '2026-01-15', '3000', 'EUR')
        price('BTC', '2026-02-15', '90000', 'EUR')
        transfer(c, 'ETH', '1', '2026-01-15T10:00:00Z', from: k.(c), to: b.(c))
        row(c, :swap_out, 'ETH', '1', '2026-02-01T10:00:00Z', exchange: b.(c), group_id: 'g', tx_id: 'g1')
        row(c, :swap_in, 'BTC', '0.01', '2026-02-01T10:00:00Z', exchange: b.(c), group_id: 'g', tx_id: 'g2')
        row(c, :swap_in, 'NOPRICE', '5', '2026-02-01T10:00:00Z', exchange: b.(c), group_id: 'g', tx_id: 'g3')
        row(c, :swap_out, 'ETH', '0.5', '2026-02-01T11:00:00Z', exchange: k.(c), group_id: 'g', tx_id: 'g4')
        row(c, :swap_in, 'BTC', '0.005', '2026-02-01T11:00:00Z', exchange: k.(c), group_id: 'g', tx_id: 'g5')
        transfer(c, 'BTC', '0.005', '2026-02-15T10:00:00Z', from: k.(c), to: b.(c))
        row(c, :sell, 'BTC', '0.015', '2026-03-01T10:00:00Z', exchange: b.(c), quote_currency: 'EUR', quote_amount: '1500')
      } },
      # A fee on the swap-in: in the received coin it nets what arrived; in a third coin it leaves that coin and joins
      # the cost.
      { name: 'at_swap_in_fees', country: 'AT', expect: 'parity', setup: lambda { |c|
        price('ETH', '2026-02-02', '3000', 'EUR')
        price('BTC', '2026-02-02', '90000', 'EUR')
        price('ETH', '2026-02-01', '3000', 'EUR')
        price('BTC', '2026-02-01', '90000', 'EUR')
        price('BNB', '2026-02-02', '500', 'EUR')
        kraken(c, :buy, 'ETH', '2', '2000', '2025-01-01T10:00:00Z', ref: 'b1')
        kraken(c, :buy, 'BNB', '1', '400', '2025-01-02T10:00:00Z', ref: 'b2')
        price('ETH', '2026-01-15', '3000', 'EUR')
        price('BNB', '2026-01-15', '500', 'EUR')
        transfer(c, 'ETH', '2', '2026-01-15T10:00:00Z', from: k.(c), to: b.(c))
        transfer(c, 'BNB', '1', '2026-01-15T11:00:00Z', from: k.(c), to: b.(c))
        row(c, :swap_out, 'ETH', '1', '2026-02-01T10:00:00Z', exchange: b.(c), group_id: 'h1', tx_id: 'h1o')
        row(c, :swap_in, 'BTC', '0.0333', '2026-02-01T10:00:00Z', exchange: b.(c), group_id: 'h1', tx_id: 'h1i', fee_currency: 'BTC', fee_amount: '0.0003')
        row(c, :swap_out, 'ETH', '1', '2026-02-02T10:00:00Z', exchange: b.(c), group_id: 'h2', tx_id: 'h2o')
        row(c, :swap_in, 'BTC', '0.0333', '2026-02-02T10:00:00Z', exchange: b.(c), group_id: 'h2', tx_id: 'h2i', fee_currency: 'BNB', fee_amount: '0.1')
        row(c, :sell, 'BTC', '0.0663', '2026-03-01T10:00:00Z', exchange: b.(c), quote_currency: 'EUR', quote_amount: '6000')
        row(c, :sell, 'BNB', '0.9', '2026-03-02T10:00:00Z', exchange: b.(c), quote_currency: 'EUR', quote_amount: '500')
      } },
      # A sweep that spends cash and a coin: the cash and the coin's carried basis make the cost.
      { name: 'at_mixed_cash_and_coin', country: 'AT', expect: 'parity', setup: lambda { |c|
        price('ETH', '2026-02-01', '3000', 'EUR')
        price('BTC', '2026-02-01', '90000', 'EUR')
        kraken(c, :buy, 'ETH', '1', '1000', '2025-01-01T10:00:00Z', ref: 'b1')
        row(c, :swap_out, 'ETH', '1', '2026-02-01T10:00:00Z', exchange: k.(c), group_id: 'm', tx_id: 'm1')
        row(c, :sell, 'EUR', '3000', '2026-02-01T10:00:00Z', exchange: k.(c), group_id: 'm', tx_id: 'm2')
        row(c, :swap_in, 'BTC', '0.0666', '2026-02-01T10:00:00Z', exchange: k.(c), group_id: 'm', tx_id: 'm3')
        kraken(c, :sell, 'BTC', '0.0666', '6500', '2026-03-01T10:00:00Z', ref: 's1')
      } },
      # A tranche older than the coin already held is sold first; unpriced units travel through the swap.
      { name: 'at_older_tranche_and_unpriced', country: 'AT', expect: 'parity', setup: lambda { |c|
        price('BTC', '2026-02-01', '90000', 'EUR')
        kraken(c, :buy, 'BTC', '0.01', '900', '2025-06-01T10:00:00Z', ref: 'b1')
        row(c, :deposit, 'ETH', '1', '2024-01-01T10:00:00Z', exchange: b.(c))
        price('BTC', '2026-01-15', '90000', 'EUR')
        transfer(c, 'BTC', '0.01', '2026-01-15T10:00:00Z', from: k.(c), to: b.(c))
        row(c, :swap_out, 'ETH', '1', '2026-02-01T10:00:00Z', exchange: b.(c), group_id: 'o', tx_id: 'o1')
        row(c, :swap_in, 'BTC', '0.03', '2026-02-01T10:00:00Z', exchange: b.(c), group_id: 'o', tx_id: 'o2')
        row(c, :sell, 'BTC', '0.02', '2026-03-01T10:00:00Z', exchange: b.(c), quote_currency: 'EUR', quote_amount: '1900')
      } },
      { name: 'de_unlinked_deposit_flags_sale', country: 'DE', expect: 'parity', setup: lambda { |c|
        price('BTC', '2026-01-10', '80000', 'EUR')
        row(c, :deposit, 'BTC', '1', '2026-01-10T10:00:00Z', exchange: b.(c))
        kraken(c, :buy, 'BTC', '1', '70000', '2026-01-11T10:00:00Z', ref: 'b1')
        row(c, :sell, 'BTC', '1', '2026-03-01T10:00:00Z', exchange: b.(c), quote_currency: 'EUR', quote_amount: '80000')
      } }
    ]
  end


  def run(scenario, dir)
    file = File.join(dir, "#{scenario[:name]}.sqlite3")
    year = scenario.fetch(:year, 2026)
    country = scenario.fetch(:country, 'US')
    stablecoin = scenario.fetch(:stablecoin, false)
    schema(file)
    travel_to(NOW) do
      ctx = build
      scenario[:setup]&.call(ctx)
      cz_prices if country == 'CZ'
      ecb(Array(scenario[:fx_gap]), country)
      dk_fx(Array(scenario[:fx_gap])) if country == 'DK'
      rails = begin
        Tax::GenerateReportJob.new.perform(ctx[:user].id, country, year, stablecoin)
        refusal = Tax::GenerateReportJob.refusal(ctx[:user].id, country, year)
        refusal ? { 'refused' => refusal } : { 'csv' => File.read(Tax::GenerateReportJob.report_path(ctx[:user].id, country, year)) }
      ensure
        FileUtils.rm_f(Tax::GenerateReportJob.report_path(ctx[:user].id, country, year))
        FileUtils.rm_f(Tax::GenerateReportJob.refusal_path(ctx[:user].id, country, year))
      end
      conn = ActiveRecord::Base.connection
      tables = TABLES.to_h { |t| [t, conn.select_all("SELECT * FROM #{t} ORDER BY id").to_a] }
      { 'name' => scenario[:name], 'country' => country, 'year' => year, 'stablecoin_as_fiat' => stablecoin, 'now' => NOW.iso8601,
        'user_id' => ctx[:user].id, 'expect' => scenario[:expect], 'tables' => tables, 'rails' => rails,
        'nbp' => (nbp = pl_default_nbp(scenario))[0], 'nbp_coverage' => nbp[1], 'pl_carry' => scenario.fetch(:pl_carry, []),
        'cnb' => (cnb = cz_default_cnb(scenario))[0], 'cnb_coverage' => cnb[1] }
    end
  end
end

out = ARGV.fetch(0)
cases = Dir.mktmpdir('tax_vectors') { |dir| TaxVectors.scenarios.map { |s| TaxVectors.run(s, dir) } }
File.write(out, JSON.pretty_generate(
  'scenarios' => cases,
  'ported_sources' => TaxVectors::PORTED.to_h { |path| [path, Digest::SHA256.file(Rails.root.join(path)).hexdigest] }
) + "\n")
puts "#{cases.size} scenarios -> #{out}"
