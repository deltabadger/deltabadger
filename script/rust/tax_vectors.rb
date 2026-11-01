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
  # The ECB's rate for every day a row was booked on, but the gaps.
  def ecb(gaps)
    days = AccountTransaction.pluck(:transacted_at).map { |t| t.utc.to_date }.uniq - gaps.map { |d| Date.parse(d) }
    rows = days.flat_map do |d|
      [{ currency: 'USD', date: d, rate: USD_PER_EUR.fetch(d.year, '1.1') }, { currency: 'PLN', date: d, rate: '4.3' }]
    end
    FxRate.insert_all(rows, unique_by: %i[currency date]) if rows.any?
  end

  def scenarios
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
    ] + us_more + fifo_jurisdictions + rails_cases
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
  def fifo_jurisdictions
    eur = lambda do |c|
      price('BTC', '2025-03-01', '60000', 'EUR')
      price('ETH', '2026-02-01', '2800', 'EUR')
      price('SOL', '2026-02-05', '140', 'EUR')
      price('BTC', '2026-02-10', '90000', 'EUR')
      kraken(c, :buy, 'BTC', '1', '60000', '2025-03-01T10:00:00Z', fee: '40')
      kraken(c, :buy, 'BTC', '0.5', '40000', '2026-01-10T10:00:00Z', fee: '20')
      row(c, :staking_reward, 'ETH', '0.01', '2026-02-01T00:00:00Z', exchange: venue(c, :kraken))
      row(c, :airdrop, 'SOL', '3', '2026-02-05T00:00:00Z', exchange: venue(c, :kraken), manual_values: { 'price' => '150.5' })
      row(c, :deposit, 'BTC', '0.1', '2026-02-10T08:00:00Z', exchange: venue(c, :kraken))
      kraken(c, :sell, 'BTC', '1.2', '108000', '2026-04-01T10:00:00Z', fee: '50')
      sell(c, 'SOL', '3', '600', '2026-04-10T08:00:00Z')
    end
    missing = lambda do |c|
      kraken(c, :buy, 'BTC', '1', '60000', '2026-01-10T10:00:00Z')
      row(c, :deposit, 'ETH', '2', '2026-02-10T08:00:00Z', exchange: venue(c, :kraken))
      kraken(c, :sell, 'ETH', '2', '5000', '2026-03-10T08:00:00Z')
      kraken(c, :sell, 'BTC', '0.5', '45000', '2026-03-10T09:00:00Z')
    end
    # A Binance Convert ETH -> BTC, then the BTC sold for euros on Kraken: a chain where swaps are not taxable.
    chain = lambda do |c|
      price('ETH', '2025-02-01', '3000', 'EUR')
      price('BTC', '2025-02-01', '90000', 'EUR')
      price('SOL', '2025-08-01', '150', 'EUR')
      price('BTC', '2025-08-01', '100000', 'EUR')
      kraken(c, :buy, 'ETH', '3', '6000', '2024-06-01T10:00:00Z')
      swap(c, 'ETH', '3', 'BTC', '0.1', '2025-02-01T10:00:00Z', ref: 'cv1')
      swap(c, 'BTC', '0.05', 'SOL', '33.3', '2025-08-01T10:00:00Z', ref: 'cv2')
      kraken(c, :sell, 'BTC', '0.05', '5500', '2026-03-01T10:00:00Z')
      kraken(c, :sell, 'SOL', '33.3', '5328', '2026-03-02T10:00:00Z')
    end
    # BTC into USDT on Binance, the USDT sold for euros on Kraken: with and without the stablecoin flag.
    stable = lambda do |c|
      price('BTC', '2026-02-01', '90000', 'EUR')
      price('USDT', '2026-02-01', '0.86', 'EUR')
      kraken(c, :buy, 'BTC', '1', '40000', '2024-06-01T10:00:00Z')
      swap(c, 'BTC', '0.5', 'USDT', '52000', '2026-02-01T10:00:00Z', ref: 'cv1')
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
        { name: "#{cc}_swap_chain", country:, expect: 'parity', setup: ->(c) { user_zone(c, zone); chain.(c) } },
        # Portugal: the USDT sold for euros on Kraken within 24 hours of the swap is the immediate fiat exit.
        # The USDT sold for euros on Kraken is valued at the euros received (and in Portugal is the immediate fiat exit).
        { name: "#{cc}_stablecoin_kept", country:, expect: 'diverges: D-TAX-STABLE-VALUE', setup: ->(c) { user_zone(c, zone); stable.(c) } },
        # Where swaps are not taxable, a stablecoin under the flag is cash: its sale for euros is no second disposal.
        { name: "#{cc}_stablecoin_as_fiat", country:, stablecoin: true,
          expect: %w[DE ES].include?(country) ? 'diverges: D-TAX-STABLE-VALUE' : 'diverges: D-TAX-STABLE-CASH', setup: ->(c) { user_zone(c, zone); stable.(c) } },
        { name: "#{cc}_ecb_gap", country:, fx_gap: %w[2026-05-15], expect: 'parity', setup: ->(c) { user_zone(c, zone); gap.(c) } },
        { name: "#{cc}_oversold", country:, expect: 'diverges: D-TAX-UNCOVERED', setup: ->(c) { user_zone(c, zone); oversold.(c) } },
        { name: "#{cc}_lost", country:, expect: 'diverges: D-TAX-LOST', setup: ->(c) { user_zone(c, zone); lost.(c) } },
        { name: "#{cc}_unlinked_withdrawal", country:, expect: 'diverges: D-TAX-UNLINKED-DISCLOSED', setup: ->(c) { user_zone(c, zone); unlinked.(c) } },
        { name: "#{cc}_coin_fee", country:, expect: 'diverges: D-TAX-COIN-FEE', setup: ->(c) { user_zone(c, zone); coin_fee.(c) } }
      ]
    end
    each.('DE', 'Berlin') + each.('AT', 'Vienna') + each.('ES', 'Madrid') + each.('PT', 'Lisbon') + each.('PL', 'Warsaw') +
      each.('SK', 'Bratislava') + holding_edges
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
        swap(c, 'ETH', '2', 'BTC', '0.0666', '2025-06-01T10:00:00Z', ref: 'cv1')
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
      # A swap-in carrying two tranches of equal worth, with a fee in BNB: the whole fee joins the new coin's cost.
      { name: 'sk_swap_fee_two_lots', country: 'SK', expect: 'diverges: D-TAX-SWAP-FEE', setup: lambda { |c|
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
      { name: 'sk_swap_fee_cash_only', country: 'SK', expect: 'diverges: D-TAX-SWAP-FEE', setup: lambda { |c|
        price('BTC', '2026-02-01', '90000', 'EUR')
        price('BNB', '2026-02-01', '500', 'EUR')
        kraken(c, :buy, 'BNB', '1', '400', '2025-01-02T10:00:00Z', ref: 'b0')
        row(c, :swap_out, 'NOPRICE', '5', '2026-02-01T10:00:00Z', exchange: venue(c, :kraken), group_id: 'cf', tx_id: 'cf-o')
        row(c, :sell, 'EUR', '100', '2026-02-01T10:00:00Z', exchange: venue(c, :kraken), group_id: 'cf', tx_id: 'cf-e')
        row(c, :swap_in, 'BTC', '0.002', '2026-02-01T10:00:00Z', exchange: venue(c, :kraken), group_id: 'cf', tx_id: 'cf-i', fee_currency: 'BNB', fee_amount: '0.02')
        kraken(c, :sell, 'BTC', '0.002', '200', '2026-03-01T10:00:00Z', ref: 's1')
      } },
      # Swap legs with nothing going the other way, where swaps are not taxable.
      { name: 'sk_orphan_swaps', country: 'SK', expect: 'diverges: D-TAX-ORPHAN-SWAP', setup: lambda { |c|
        price('ETH', '2026-02-01', '3000', 'EUR')
        price('BTC', '2026-02-03', '90000', 'EUR')
        kraken(c, :buy, 'ETH', '1', '1000', '2025-01-03T10:00:00Z', ref: 'b1')
        row(c, :swap_out, 'ETH', '1', '2026-02-01T10:00:00Z', exchange: venue(c, :binance), group_id: 'o1', tx_id: 'o1-o')
        row(c, :swap_in, 'BTC', '0.03', '2026-02-03T10:00:00Z', exchange: venue(c, :binance), group_id: 'o2', tx_id: 'o2-i')
        row(c, :sell, 'BTC', '0.03', '2026-03-01T10:00:00Z', exchange: venue(c, :binance), quote_currency: 'EUR', quote_amount: '2900')
      } },
      # Staking received at 00:30 on 1 January in Berlin is that year's income.
      { name: 'de_income_new_year', country: 'DE', expect: 'diverges: D-TAX-LOCAL-YEAR', setup: lambda { |c|
        user_zone(c, 'Berlin')
        price('ETH', '2025-12-31', '3000', 'EUR')
        row(c, :staking_reward, 'ETH', '0.1', '2025-12-31T23:30:00Z', exchange: venue(c, :kraken))
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
        row(c, :sell, 'BTC', '1.5', '2026-03-01T10:00:00Z', exchange: b.(c), quote_currency: 'EUR', quote_amount: '120000')
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
        row(c, :swap_out, 'ETH', '1', '2026-02-01T10:00:00Z', exchange: b.(c), group_id: 'g', tx_id: 'g1')
        row(c, :swap_in, 'BTC', '0.01', '2026-02-01T10:00:00Z', exchange: b.(c), group_id: 'g', tx_id: 'g2')
        row(c, :swap_in, 'NOPRICE', '5', '2026-02-01T10:00:00Z', exchange: b.(c), group_id: 'g', tx_id: 'g3')
        row(c, :swap_out, 'ETH', '0.5', '2026-02-01T11:00:00Z', exchange: k.(c), group_id: 'g', tx_id: 'g4')
        row(c, :swap_in, 'BTC', '0.005', '2026-02-01T11:00:00Z', exchange: k.(c), group_id: 'g', tx_id: 'g5')
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
        row(c, :swap_out, 'ETH', '1', '2026-02-01T10:00:00Z', exchange: b.(c), group_id: 'o', tx_id: 'o1')
        row(c, :swap_in, 'BTC', '0.03', '2026-02-01T10:00:00Z', exchange: b.(c), group_id: 'o', tx_id: 'o2')
        row(c, :sell, 'BTC', '0.02', '2026-03-01T10:00:00Z', exchange: b.(c), quote_currency: 'EUR', quote_amount: '1900')
      } },
      { name: 'de_unlinked_deposit_flags_sale', country: 'DE', expect: 'parity', setup: lambda { |c|
        price('BTC', '2026-01-10', '80000', 'EUR')
        row(c, :deposit, 'BTC', '1', '2026-01-10T10:00:00Z', exchange: b.(c))
        kraken(c, :buy, 'BTC', '1', '70000', '2026-01-11T10:00:00Z', ref: 'b1')
        row(c, :sell, 'BTC', '1.5', '2026-03-01T10:00:00Z', exchange: b.(c), quote_currency: 'EUR', quote_amount: '120000')
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
      ecb(Array(scenario[:fx_gap]))
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
        'user_id' => ctx[:user].id, 'expect' => scenario[:expect], 'tables' => tables, 'rails' => rails }
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
