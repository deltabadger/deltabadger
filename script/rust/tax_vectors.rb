# The Rails half of the tax report's parity (rust/tests/tax_parity.rs is the Rust half).
#   bin/rails runner script/rust/tax_vectors.rb rust/tests/fixtures/tax_vectors.json
# Run from a clean export (git archive), never a working install: each scenario is built on its own scratch SQLite file
# with the app's own models, Tax::GenerateReportJob#perform runs exactly as the queue would run it, and the file it
# writes (or the refusal beside it) is recorded with every table row the Rust half reads. Re-run when a file under
# "ported_sources" changes.
#
# Each scenario says what Rust must do with it: "parity" (the same bytes, or the same refusal), or "refuses: <reason>"
# where Rails states a figure that cannot be defended (a defect of the Rails report).
require 'json'
require 'digest'
require 'fileutils'
require 'tmpdir'
require 'active_support/testing/time_helpers'

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
  TABLES = %w[users exchanges assets tickers api_keys account_transactions historical_prices fund_classifications].freeze
  PORTED = %w[
    app/jobs/tax/generate_report_job.rb app/models/tax/report.rb app/models/tax/methods/fifo.rb
    app/models/tax/methods/acquisition_fee.rb app/models/tax/price_service.rb app/models/tax/crypto_scope.rb
    app/models/tax/asset_identity.rb app/models/tax/jurisdictions.rb app/models/api_key.rb app/models/concerns/csv_safe.rb
  ].freeze

  # The schema is loaded before travelling: a migration stamped after NOW would not load.
  def schema(file)
    ActiveRecord::Base.establish_connection(adapter: 'sqlite3', database: file)
    ActiveRecord::Schema.verbose = false
    load Rails.root.join('db/schema.rb')
  end

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
  def price(symbol, day, value) = HistoricalPrice.create!(asset: symbol, currency: 'USD', date: Date.parse(day), price: value)

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
      { name: 'missing_price', expect: 'refuses: no price of BTC', setup: lambda { |c|
        row(c, :deposit, 'BTC', '0.1', '2026-02-10T08:00:00Z')
        sell(c, 'BTC', '0.1', '9000', '2026-03-10T08:00:00Z')
      } },
      { name: 'oversold', expect: 'refuses: a sale of more than the history holds', setup: lambda { |c|
        buy(c, 'BTC', '1', '100', '2026-01-05T10:00:00Z')
        sell(c, 'BTC', '2', '300', '2026-01-06T10:00:00Z')
      } },
      { name: 'never_held', expect: 'refuses: a sale of more than the history holds', setup: lambda { |c|
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
      { name: 'cfee_calendars_disagree', expect: 'refuses: a fee paid in a coin', setup: lambda { |c|
        price('BTC', '2026-01-05', '95000')
        buy(c, 'BTC', '1', '100', '2026-01-05T10:00:00Z')
        buy(c, 'BTC', '3', '330', '2026-01-06T02:00:00Z')
        row(c, :fee, 'BTC', '0.0025', '2026-01-05T00:00:00Z', description: 'CFEE', raw_data: { 'activity_type' => 'CFEE' })
        sell(c, 'BTC', '0.9975', '300', '2026-01-06T10:00:00Z')
      } },
      { name: 'fee_of_no_trade', expect: 'refuses: a fee paid in a coin', setup: lambda { |c|
        price('BTC', '2026-01-09', '95000')
        buy(c, 'BTC', '1', '100', '2026-01-05T10:00:00Z')
        row(c, :fee, 'BTC', '0.0025', '2026-01-09T00:00:00Z', description: 'CFEE', raw_data: { 'activity_type' => 'CFEE' })
        sell(c, 'BTC', '0.9975', '300', '2026-01-10T10:00:00Z')
      } },
      { name: 'unlinked_withdrawal', expect: 'refuses: a coin withdrawal not linked', setup: lambda { |c|
        price('BTC', '2026-01-06', '95000')
        buy(c, 'BTC', '1', '100', '2026-01-05T10:00:00Z')
        row(c, :withdrawal, 'BTC', '0.5', '2026-01-06T10:00:00Z')
        sell(c, 'BTC', '0.5', '300', '2026-01-07T10:00:00Z')
      } },
      { name: 'lost', expect: 'refuses: a lost coin', setup: lambda { |c|
        price('BTC', '2026-01-06', '95000')
        buy(c, 'BTC', '1', '100', '2026-01-05T10:00:00Z')
        row(c, :lost, 'BTC', '0.5', '2026-01-06T10:00:00Z')
        sell(c, 'BTC', '0.5', '300', '2026-01-07T10:00:00Z')
      } },
      { name: 'unsupported', expect: 'refuses: a corporate action, split or return of capital', setup: lambda { |c|
        buy(c, 'BTC', '1', '100', '2026-01-05T10:00:00Z')
        row(c, :unsupported_activity, 'BTC', '1', '2026-01-06T10:00:00Z')
      } },
      { name: 'other_venue', expect: 'refuses: rows of a venue other than Alpaca', setup: lambda { |c|
        kraken = Exchanges::Kraken.create!(name: 'Kraken', maker_fee: '0.25', taker_fee: '0.4')
        buy(c, 'BTC', '1', '100', '2026-01-05T10:00:00Z', exchange: kraken)
      } }
    ]
  end

  def run(scenario, dir)
    file = File.join(dir, "#{scenario[:name]}.sqlite3")
    year = scenario.fetch(:year, 2026)
    schema(file)
    travel_to(NOW) do
      ctx = build
      scenario[:setup]&.call(ctx)
      rails = begin
        Tax::GenerateReportJob.new.perform(ctx[:user].id, 'US', year)
        refusal = Tax::GenerateReportJob.refusal(ctx[:user].id, 'US', year)
        refusal ? { 'refused' => refusal } : { 'csv' => File.read(Tax::GenerateReportJob.report_path(ctx[:user].id, 'US', year)) }
      ensure
        FileUtils.rm_f(Tax::GenerateReportJob.report_path(ctx[:user].id, 'US', year))
        FileUtils.rm_f(Tax::GenerateReportJob.refusal_path(ctx[:user].id, 'US', year))
      end
      conn = ActiveRecord::Base.connection
      tables = TABLES.to_h { |t| [t, conn.select_all("SELECT * FROM #{t} ORDER BY id").to_a] }
      { 'name' => scenario[:name], 'year' => year, 'now' => NOW.iso8601, 'user_id' => ctx[:user].id,
        'expect' => scenario[:expect], 'tables' => tables, 'rails' => rails }
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
