# The Rails half of the tracker's value-parity harness (rust/tests/tracker_parity.rs is the Rust half).
#   bin/rails runner script/rust/tracker.rb grid <root>    # one install per scenario, built with Rails' own models
#   bin/rails runner script/rust/tracker.rb record <root>  # Rails' real jobs per <root>/<scenario>/ -> rails.json
# Always run with every *_DATABASE_URL pointing at scratch files. data-api and Alpaca's market data are scripted beneath
# Rails' own clients, at the Faraday adapter (the contract of script/rust/sync.rb): every line of Tracker::LedgerJob,
# Tracker::Ledger, Tracker::Figures, PortfolioSnapshot, PortfolioSnapshot::BackfillJob and Tax::PriceService runs. A
# call a scenario did not script raises Harness::Unscripted, and so does any other real connection.
require 'json'
require 'fileutils'
require 'active_support/testing/time_helpers'

# Tax::PriceService#fetch_price_range asks data-api from `Date#to_time`, which is the HOST's zone: hosted containers
# run in UTC, and so does this harness.
ENV['TZ'] = 'UTC'
# A hosted instance: prices that are neither cash nor Alpaca's own come from the market-data API (MarketDataSettings).
ENV['MARKET_DATA_URL'] = 'http://data-api:3000'
ENV['MARKET_DATA_TOKEN'] = 'parity-token'
ENV['COINGECKO_API_KEY'] = ''

module Harness
  # An Exception, not a StandardError: the jobs' own rescues would turn a harness gap into a missing price.
  class Unscripted < Exception; end # rubocop:disable Lint/InheritException
end

Net::HTTP.prepend(Module.new { def connect = raise(Harness::Unscripted, "real connection to #{address}:#{port}") })

module ScriptedTracker
  mattr_accessor :alpaca, :market, :requests

  def self.reply(env)
    host = env.url.host.to_s
    kind, script = if host.end_with?('alpaca.markets') then ['alpaca', alpaca]
                   elsif host == 'data-api' then ['market', market]
                   end
    key = "#{env.method.to_s.upcase} #{env.url.path}"
    raise Harness::Unscripted, "unscripted HTTP call #{key} to #{host}" if script.nil?

    requests[kind] << [key, URI.decode_www_form(env.url.query.to_s).sort]
    queue = script[key] or raise Harness::Unscripted, "unscripted call #{key}"
    queue.size > 1 ? queue.shift : queue.first
  end

  module Adapter
    def call(env)
      reply = ScriptedTracker.reply(env)
      body = reply['body'].is_a?(String) ? reply['body'] : JSON.generate(reply['body'])
      env.response = Faraday::Response.new
      save_response(env, reply.fetch('status', 200), body, { 'Content-Type' => 'application/json' })
      @app.call(env)
    end
  end
end

module TrackerParity
  extend ActiveSupport::Testing::TimeHelpers
  module_function

  KEY = { 'key' => 'PKPARITY7KEY4TEST2ID', 'secret' => 'parity-secret-Zq8fW3nL5xT1vB7mK2dR9hY4', 'passphrase' => 'paper' }.freeze
  # Thursday: the backfill sweeps to Wednesday 30 September; the wash-sale horizon starts on 31 August.
  AT = '2026-10-01T03:00:00Z'.freeze
  SYNCED = Time.utc(2026, 10, 1, 2, 30, 0)
  HISTORY = 'GET /api/v1/historical_prices'.freeze
  ENTRY = AccountTransaction.entry_types

  def ok(body) = { 'status' => 200, 'body' => body }
  def raw(value) = value.is_a?(Float) ? { 'f' => [value].pack('G').unpack1('H*') } : value
  def ms(day) = Time.utc(*day.split('-').map(&:to_i)).to_i * 1000

  # data-api's historical prices answer: [[epoch ms, price], ...].
  def history(prices) = ok('prices' => prices.map { |day, price| [ms(day), price] })

  # An Alpaca daily-bars answer.
  def bars(symbol, closes)
    ok('bars' => closes.map { |day, c| { 't' => "#{day}T04:00:00Z", 'o' => c, 'h' => c, 'l' => c, 'c' => c, 'v' => 1000, 'n' => 10, 'vw' => c } },
       'symbol' => symbol, 'next_page_token' => nil)
  end

  def build(dir, scenario)
    { 'production.sqlite3' => 'db/schema.rb', 'production_queue.sqlite3' => 'db/queue_schema.rb' }.each do |file, schema|
      ActiveRecord::Base.establish_connection(adapter: 'sqlite3', database: File.join(dir, file))
      ActiveRecord::Schema.verbose = false
      load Rails.root.join(schema)
    end
    ActiveRecord::Base.establish_connection(adapter: 'sqlite3', database: File.join(dir, 'production.sqlite3'))
    travel_to(Time.utc(2026, 9, 1, 9, 0, 0)) do
      user = User.new(name: 'Owner', email: 'owner@example.com', password: 'correct horse battery staple', admin: true,
                      confirmed_at: Time.current, setup_completed: true)
      user.save!(validate: false)
      alpaca = Exchanges::Alpaca.create!(name: 'Alpaca', maker_fee: '0.15', taker_fee: '0.25')
      assets = {
        'USD' => Asset.create!(external_id: 'usd', symbol: 'USD', name: 'US Dollar', category: 'Fiat'),
        'AAPL' => Asset.create!(external_id: 'AAPL.US', symbol: 'AAPL', name: 'Apple', category: 'Stock'),
        'KLAC' => Asset.create!(external_id: 'KLAC.US', symbol: 'KLAC', name: 'KLA', category: 'Stock'),
        'QQQM' => Asset.create!(external_id: 'QQQM.US', symbol: 'QQQM', name: 'Invesco NASDAQ 100', category: 'Stock'),
        'BTC' => Asset.create!(external_id: 'bitcoin', symbol: 'BTC', name: 'Bitcoin', category: 'Cryptocurrency'),
        'ETH' => Asset.create!(external_id: 'ethereum', symbol: 'ETH', name: 'Ethereum', category: 'Cryptocurrency'),
        'USDC' => Asset.create!(external_id: 'usd-coin', symbol: 'USDC', name: 'USD Coin', category: 'Cryptocurrency')
      }
      assets.each_value { |a| ExchangeAsset.create!(exchange: alpaca, asset: a, available: true) }
      stock = { base_decimals: 9, quote_decimals: 2, price_decimals: 2, minimum_base_size: '0.000000001', minimum_quote_size: '1' }
      %w[AAPL KLAC QQQM].each do |s|
        Ticker.create!(exchange: alpaca, ticker: s, base: s, quote: 'USD', base_asset: assets[s], quote_asset: assets['USD'], **stock)
      end
      %w[BTC ETH USDC].each do |s|
        Ticker.create!(exchange: alpaca, ticker: "#{s}/USD", base: s, quote: 'USD', base_asset: assets[s], quote_asset: assets['USD'], **stock)
      end
      key = ApiKey.new(user:, exchange: alpaca, **KEY.symbolize_keys, status: :correct, key_type: :trading, balances_synced_at: SYNCED,
                       last_synced_at: SYNCED)
      key.save!(validate: false)
      # Stored plain (read as itself under support_unencrypted_data), so the Rust half reads the key without Rails' secret.
      ActiveRecord::Base.connection.exec_update('UPDATE api_keys SET key = ?, secret = ?, passphrase = ? WHERE id = ?', 'plain credentials',
                                                [KEY['key'], KEY['secret'], KEY['passphrase'], key.id])
      # The ECB table is current, so Tax::EcbFxRates.ensure_loaded! fetches nothing (the harness has no ECB script).
      FxRate.create!(currency: 'USD', date: Date.new(2026, 9, 30), rate: '1.17')
      ctx = { user:, alpaca:, assets:, key: }
      scenario[:setup]&.call(ctx)
      ctx
    end
  end

  def row(ctx, type, base, amount, at, **attrs)
    AccountTransaction.insert!({ user_id: ctx[:user].id, exchange_id: ctx[:alpaca].id, api_key_id: ctx[:key].id, entry_type: ENTRY.fetch(type.to_s),
                                 base_currency: base, base_amount: amount, transacted_at: Time.iso8601(at), raw_data: {}, manual_values: {},
                                 tx_id: attrs.delete(:tx_id) || "#{type}-#{base}-#{at}", created_at: Time.current, updated_at: Time.current }.merge(attrs))
    AccountTransaction.order(:id).last
  end

  def buy(ctx, symbol, qty, price, at) = row(ctx, :buy, symbol, qty, at, quote_currency: 'USD', quote_amount: (qty.to_d * price.to_d).to_s('F'))
  def sell(ctx, symbol, qty, price, at) = row(ctx, :sell, symbol, qty, at, quote_currency: 'USD', quote_amount: (qty.to_d * price.to_d).to_s('F'))
  def deposit(ctx, amount, at) = row(ctx, :deposit, 'USD', amount, at)

  def balance(ctx, symbol, free, usd_price: nil, usd_value: :auto, priced_at: SYNCED, synced_at: SYNCED, exchange: ctx[:alpaca])
    usd_value = usd_price && (free.to_d * usd_price.to_d).round(8) if usd_value == :auto
    AccountBalance.insert!({ user_id: ctx[:user].id, exchange_id: exchange.id, asset_id: ctx[:assets].fetch(symbol).id, free:, locked: 0,
                             usd_price:, usd_value:, priced_at: usd_price && priced_at, synced_at:, created_at: synced_at, updated_at: synced_at })
  end

  # Closes for the weekdays of September from `from`, rising a dollar a day from `start`, except the days in `holes`.
  def september(from, start, holes: [])
    (Date.parse(from)..Date.new(2026, 9, 30)).reject { |d| d.saturday? || d.sunday? || holes.include?(d.to_s) }
                                              .each_with_index.to_h { |d, i| [d.to_s, start + i] }
  end

  def scenarios
    aapl_bars = { 'GET /v2/stocks/AAPL/bars' => [bars('AAPL', september('2026-09-02', 200))] }
    [
      { name: 'empty' },
      { name: 'buys_priced', alpaca: { 'GET /v2/stocks/AAPL/bars' => [bars('AAPL', september('2026-09-02', 200, holes: (8..21).map { |d| format('2026-09-%02d', d) }))] },
        setup: lambda { |c|
          deposit(c, '1000', '2026-09-01T14:00:00Z')
          buy(c, 'AAPL', '2', '200', '2026-09-02T14:30:00Z')
          buy(c, 'AAPL', '1', '210.5', '2026-09-15T14:30:00Z')
          balance(c, 'USD', '389.5', usd_price: '1')
          balance(c, 'AAPL', '3', usd_price: '228.37')
        } },
      { name: 'sell_at_loss_wash_on', alpaca: aapl_bars,
        setup: lambda { |c|
          c[:user].update_columns(wash_sale_enabled: true)
          deposit(c, '1000', '2026-09-01T14:00:00Z')
          buy(c, 'AAPL', '2', '200', '2026-09-02T14:30:00Z')
          sell(c, 'AAPL', '1', '150', '2026-09-20T14:30:00Z')
          balance(c, 'USD', '750', usd_price: '1')
          balance(c, 'AAPL', '1', usd_price: '160')
        } },
      { name: 'sell_at_loss_wash_off', alpaca: aapl_bars,
        setup: lambda { |c|
          c[:user].update_columns(wash_sale_enabled: false)
          deposit(c, '1000', '2026-09-01T14:00:00Z')
          buy(c, 'AAPL', '2', '200', '2026-09-02T14:30:00Z')
          sell(c, 'AAPL', '1', '150', '2026-09-20T14:30:00Z')
          balance(c, 'USD', '750', usd_price: '1')
          balance(c, 'AAPL', '1', usd_price: '160')
        } },
      { name: 'sale_gain_with_a_losing_lot', alpaca: { 'GET /v2/stocks/AAPL/bars' => [bars('AAPL', september('2026-09-01', 100))] },
        setup: lambda { |c|
          c[:user].update_columns(wash_sale_enabled: true, wash_sale_jurisdiction: 'IE')
          # A lock already longer than this sale earns: raised to nothing, the confirmed deadline set.
          WashSaleLock.create!(user: c[:user], asset: c[:assets]['AAPL'], buy_locked_until: Time.utc(2026, 12, 1), source: 'bot')
          deposit(c, '1000', '2026-09-01T13:00:00Z')
          buy(c, 'AAPL', '1', '100', '2026-09-01T14:30:00Z')
          buy(c, 'AAPL', '1', '300', '2026-09-05T14:30:00Z')
          sell(c, 'AAPL', '2', '250', '2026-09-25T14:30:00Z')
          balance(c, 'USD', '1100', usd_price: '1')
        } },
      { name: 'old_loss_outside_the_horizon', alpaca: { 'GET /v2/stocks/AAPL/bars' => [bars('AAPL', september('2026-08-03', 150))] },
        setup: lambda { |c|
          c[:user].update_columns(wash_sale_enabled: true)
          deposit(c, '1000', '2026-08-03T13:00:00Z')
          buy(c, 'AAPL', '2', '200', '2026-08-03T14:30:00Z')
          sell(c, 'AAPL', '1', '150', '2026-08-28T14:30:00Z')
          balance(c, 'USD', '750', usd_price: '1')
          balance(c, 'AAPL', '1', usd_price: '160')
        } },
      { name: 'dividends_fees_withdrawal', alpaca: { 'GET /v2/stocks/QQQM/bars' => [bars('QQQM', september('2026-09-02', 500))] },
        setup: lambda { |c|
          deposit(c, '1000', '2026-09-01T14:00:00Z')
          buy(c, 'QQQM', '1', '500', '2026-09-02T14:30:00Z')
          row(c, :other_income, 'USD', '5', '2026-09-10T00:00:00Z', quote_currency: 'QQQM', group_id: 'div-1', description: 'Dividend (QQQM)')
          row(c, :withholding_tax, 'USD', '0.75', '2026-09-10T00:00:00Z', quote_currency: 'QQQM', group_id: 'div-1')
          row(c, :fee, 'USD', '1', '2026-09-11T00:00:00Z', group_id: 'fee-1')
          row(c, :withdrawal, 'USD', '100', '2026-09-12T00:00:00Z')
          balance(c, 'USD', '400', usd_price: '1')
          balance(c, 'QQQM', '1', usd_price: '520.125')
        } },
      { name: 'split_priced_by_a_second_fetch',
        market: { HISTORY => [history([]), history([['2026-09-14', 99.5], ['2026-09-15', 10.25], ['2026-09-16', 10.5]])] },
        alpaca: { 'GET /v2/stocks/KLAC/bars' => [bars('KLAC', september('2026-09-02', 100).map { |d, p| [d, d < '2026-09-15' ? p : p / 10.0] })] },
        setup: lambda { |c|
          deposit(c, '2000', '2026-09-01T14:00:00Z')
          buy(c, 'KLAC', '10', '100', '2026-09-02T14:30:00Z')
          row(c, :adjustment, 'KLAC', '90', '2026-09-15T00:00:00Z', raw_data: { 'corporate_action' => 'split', 'merged_activity_ids' => %w[a b], 'split_ratio' => '10:1' })
          balance(c, 'USD', '1000', usd_price: '1')
          balance(c, 'KLAC', '100', usd_price: '11.875')
        } },
      { name: 'coin_fee_unpriced', market: { HISTORY => [history([])] },
        setup: lambda { |c|
          deposit(c, '1000', '2026-09-01T14:00:00Z')
          buy(c, 'BTC', '0.01', '60000', '2026-09-03T10:00:00Z')
          row(c, :fee, 'BTC', '0.0001', '2026-09-10T00:00:00Z', description: 'CFEE')
          balance(c, 'USD', '400', usd_price: '1')
          balance(c, 'BTC', '0.0099', usd_price: '61000')
        } },
      # A coin fee takes its own released basis off the queue (coin_cost), so the later withdrawal takes $20, not $10.
      { name: 'coin_fee_then_withdrawal',
        market: { HISTORY => [history([['2026-09-03', 100], ['2026-09-04', 150], ['2026-09-05', 200]])] },
        setup: lambda { |c|
          deposit(c, '100', '2026-09-01T14:00:00Z')
          buy(c, 'BTC', '0.1', '100', '2026-09-02T14:00:00Z')
          row(c, :fee, 'BTC', '0.1', '2026-09-03T14:00:00Z')
          buy(c, 'BTC', '0.1', '200', '2026-09-04T14:00:00Z')
          row(c, :withdrawal, 'BTC', '0.1', '2026-09-05T14:00:00Z')
          balance(c, 'USD', '70', usd_price: '1')
        } },
      { name: 'coin_fee_stated_price',
        market: { HISTORY => [history([['2026-09-03', 60_100.5], ['2026-09-04', 60_200], ['2026-09-30', 61_000.25]])] },
        setup: lambda { |c|
          deposit(c, '1000', '2026-09-01T14:00:00Z')
          buy(c, 'BTC', '0.01', '60000', '2026-09-03T10:00:00Z')
          row(c, :fee, 'BTC', '0.0001', '2026-09-10T00:00:00Z', manual_values: { 'price' => '59000' })
          balance(c, 'USD', '400', usd_price: '1')
          balance(c, 'BTC', '0.0099', usd_price: '61000')
        } },
      { name: 'bought_since_the_sync', alpaca: aapl_bars,
        setup: lambda { |c|
          c[:key].update_columns(balances_synced_at: Time.utc(2026, 9, 30, 2, 30))
          deposit(c, '1000', '2026-09-01T14:00:00Z')
          buy(c, 'AAPL', '2', '200', '2026-09-02T14:30:00Z')
          buy(c, 'AAPL', '1', '210', '2026-09-30T15:00:00Z')
          buy(c, 'ETH', '0.1', '2500', '2026-09-30T16:00:00Z')
          balance(c, 'USD', '600', usd_price: '1', synced_at: Time.utc(2026, 9, 30, 2, 30), priced_at: Time.utc(2026, 9, 30, 2, 30))
          balance(c, 'AAPL', '2', usd_price: '205', synced_at: Time.utc(2026, 9, 30, 2, 30), priced_at: Time.utc(2026, 9, 30, 2, 30))
        },
        market: { HISTORY => [history([['2026-09-30', 2510]])] } },
      { name: 'departed_and_cash_short', alpaca: aapl_bars,
        market: { HISTORY => [history((1..30).map { |d| [format('2026-09-%02d', d), 2400 + d] })] },
        setup: lambda { |c|
          deposit(c, '1000', '2026-09-01T14:00:00Z')
          buy(c, 'AAPL', '2', '200', '2026-09-02T14:30:00Z')
          buy(c, 'ETH', '0.1', '2400', '2026-09-03T10:00:00Z')
          balance(c, 'USD', '300', usd_price: '1')
          balance(c, 'AAPL', '1', usd_price: '215')
        } },
      { name: 'cash_beyond_the_history_and_an_unpriced_holding', alpaca: aapl_bars,
        setup: lambda { |c|
          deposit(c, '1000', '2026-09-01T14:00:00Z')
          buy(c, 'AAPL', '2', '200', '2026-09-02T14:30:00Z')
          balance(c, 'USD', '700', usd_price: '1')
          balance(c, 'AAPL', '2', usd_price: nil)
          balance(c, 'QQQM', '1', usd_price: '510')
        } },
      { name: 'a_failed_sync_and_stale_prices', alpaca: aapl_bars,
        setup: lambda { |c|
          c[:key].update_columns(last_sync_error: 'Faraday::TimeoutError: Net::ReadTimeout')
          deposit(c, '1000', '2026-09-01T14:00:00Z')
          buy(c, 'AAPL', '2', '200', '2026-09-02T14:30:00Z')
          balance(c, 'USD', '600', usd_price: '1')
          balance(c, 'AAPL', '2', usd_price: '210', priced_at: SYNCED - 600)
        } },
      { name: 'stale_prices_alone', alpaca: aapl_bars,
        setup: lambda { |c|
          deposit(c, '1000', '2026-09-01T14:00:00Z')
          buy(c, 'AAPL', '2', '200', '2026-09-02T14:30:00Z')
          balance(c, 'USD', '600', usd_price: '1')
          balance(c, 'AAPL', '2', usd_price: '210', priced_at: SYNCED - 301)
        } },
      { name: 'linked_dollars',
        setup: lambda { |c|
          deposit(c, '1000', '2026-09-01T14:00:00Z')
          d = deposit(c, '99', '2026-09-06T14:00:00Z')
          row(c, :withdrawal, 'USD', '100', '2026-09-05T14:00:00Z', linked_transaction_id: d.id)
          balance(c, 'USD', '999', usd_price: '1')
        } },
      { name: 'sold_before_any_buy',
        market: { HISTORY => [history([['2026-09-09', 190.25], ['2026-09-10', 191]])] },
        alpaca: { 'GET /v2/stocks/AAPL/bars' => [bars('AAPL', september('2026-09-10', 190))] },
        setup: lambda { |c|
          sell(c, 'AAPL', '1', '200', '2026-09-10T00:00:00Z')
          buy(c, 'AAPL', '3', '195', '2026-09-11T14:30:00Z')
          balance(c, 'USD', '0', usd_price: '1')
          balance(c, 'AAPL', '3', usd_price: '205')
        } },
      # The opening's first fetch comes back empty, the second prices it above the sale: a loss, and its lock.
      { name: 'a_failed_price_fetched_again',
        market: { HISTORY => [history([]), history([['2026-09-09', 250]])] },
        alpaca: { 'GET /v2/stocks/AAPL/bars' => [bars('AAPL', september('2026-09-10', 190))] },
        setup: lambda { |c|
          c[:user].update_columns(wash_sale_enabled: true)
          sell(c, 'AAPL', '1', '200', '2026-09-10T00:00:00Z')
          buy(c, 'AAPL', '3', '195', '2026-09-11T14:30:00Z')
          balance(c, 'USD', '0', usd_price: '1')
          balance(c, 'AAPL', '3', usd_price: '205')
        } },
      { name: 'return_of_capital', alpaca: aapl_bars,
        setup: lambda { |c|
          deposit(c, '2000', '2026-09-01T14:00:00Z')
          buy(c, 'AAPL', '10', '100', '2026-09-02T14:30:00Z')
          row(c, :return_of_capital, 'AAPL', '10', '2026-09-20T00:00:00Z', quote_currency: 'USD', quote_amount: '5',
                                                     raw_data: { 'per_share_amount' => '0.5', 'activity_type' => 'DIVROC' })
          balance(c, 'USD', '1005', usd_price: '1')
          balance(c, 'AAPL', '10', usd_price: '101')
        } },
      { name: 'refused_non_cash_quote', alpaca: aapl_bars, market: { HISTORY => [history([['2026-09-02', 201]])] },
        setup: lambda { |c|
          deposit(c, '1000', '2026-09-01T14:00:00Z')
          row(c, :buy, 'AAPL', '2', '2026-09-02T14:30:00Z', quote_amount: '400')
          balance(c, 'USD', '600', usd_price: '1')
          balance(c, 'AAPL', '2', usd_price: '210')
        } },
      { name: 'refused_other_venue_balance', alpaca: aapl_bars,
        setup: lambda { |c|
          deposit(c, '1000', '2026-09-01T14:00:00Z')
          buy(c, 'AAPL', '2', '200', '2026-09-02T14:30:00Z')
          balance(c, 'USD', '600', usd_price: '1')
          balance(c, 'AAPL', '2', usd_price: '210')
          balance(c, 'BTC', '0.01', usd_price: '61000', exchange: Exchanges::Binance.create!(name: 'Binance', maker_fee: '0.1', taker_fee: '0.1'))
        } },
      # A coin no catalogue names: Rails values it at zero without a request (a partial day).
      { name: 'refused_unnamed_coin', alpaca: aapl_bars,
        setup: lambda { |c|
          deposit(c, '1000', '2026-09-01T14:00:00Z')
          buy(c, 'AAPL', '2', '200', '2026-09-02T14:30:00Z')
          row(c, :airdrop, 'XYZ', '5', '2026-09-10T00:00:00Z')
          balance(c, 'USD', '600', usd_price: '1')
          balance(c, 'AAPL', '2', usd_price: '210')
        } },
      { name: 'refused_swap_legs', market: { HISTORY => [history([])] },
        setup: lambda { |c|
          deposit(c, '1000', '2026-09-01T14:00:00Z')
          row(c, :swap_out, 'USD', '100', '2026-09-02T14:30:00Z', group_id: 'sw-1')
          row(c, :swap_in, 'USDC', '100', '2026-09-02T14:30:00Z', group_id: 'sw-1')
          balance(c, 'USD', '900', usd_price: '1')
        } }
    ]
  end

  def grid(root)
    list = scenarios
    list.each do |sc|
      dir = File.join(root, sc[:name])
      FileUtils.mkdir_p(dir)
      ctx = build(dir, sc)
      File.write(File.join(dir, 'scenario.json'),
                 JSON.pretty_generate('parity_scratch' => true, 'user_id' => ctx[:user].id, 'at' => AT,
                                      'market' => sc[:market] || {}, 'alpaca' => sc[:alpaca] || {}))
      ActiveRecord::Base.connection_pool.disconnect!
    end
    puts "built #{list.size} scenarios in #{root}"
  end

  def d(value) = value&.to_d&.to_s('F')

  def summary(s)
    { 'positions' => s.positions.sort_by(&:symbol).map do |p|
        { 'symbol' => p.symbol, 'quantity' => d(p.quantity), 'cost' => d(p.cost_usd), 'avg_cost' => d(p.avg_cost_usd),
          'estimated' => p.estimated ? true : false, 'unpriced' => d(p.unpriced_quantity) }
      end,
      'total_invested' => d(s.total_invested_usd), 'cash' => s.cash.map { |c, a| [c, d(a)] }, 'cash_basis' => s.cash_basis.map { |c, a| [c, d(a)] },
      'incomplete' => s.incomplete ? true : false, 'loss_sales' => s.loss_sales.map { |sym, on| [sym, on.to_s] } }
  end

  def figures(f)
    positions = f.without_cash
    { 'value' => d(f.value), 'invested' => d(f.invested), 'held_value' => d(positions.value), 'held_cost' => d(positions.invested),
      'holdings' => f.holdings.sort_by { |h| h.asset.symbol }.map do |h|
        { 'symbol' => h.asset.symbol, 'quantity' => d(h.quantity), 'value' => d(h.value), 'cost' => d(h.cost) }
      end }
  end

  # The scopes the job cached, and the figures each of today's rows was made from (PortfolioSnapshot.today_row).
  def report(user)
    scopes = Rails.cache.read(Tracker::Ledger.send(:cache_key, user))
    figs = [nil, *PortfolioSnapshot.venues(user)].each_with_object({}) do |exchange, out|
      balances = AccountBalance.for_user(user).nonzero.then { |s| exchange ? s.for_exchange(exchange) : s }.to_a
      pending_scope = PortfolioSnapshot.pending_scope(user, exchange)
      next if balances.empty? && !pending_scope.exists?

      ledger = Tracker::Ledger.summary(user, exchange: exchange, scopes: scopes)
      pending = Tracker::Figures.moved_since(pending_scope, PortfolioSnapshot.watermarks(user, exchange))
      out[exchange ? exchange.id.to_s : 'whole'] = figures(Tracker::Figures.for(user, ledger: ledger, balances: balances, pending: pending))
    end
    { 'whole' => summary(scopes[nil]), 'venues' => scopes.except(nil).to_h { |id, s| [id.to_s, summary(s)] }, 'figures' => figs }
  end

  def tables(user)
    out = { 'portfolio_snapshots' => 'date', 'portfolio_venue_snapshots' => 'exchange_id, date', 'wash_sale_locks' => 'id', 'historical_prices' => 'id' }
          .to_h do |table, order|
      rows = ActiveRecord::Base.connection.select_all("SELECT * FROM #{table} ORDER BY #{order}").to_a.map do |r|
        r = r.except('id', 'created_at', 'updated_at') if table.start_with?('portfolio')
        r.transform_values { |v| raw(v) }
      end
      [table, rows]
    end
    [PortfolioSnapshot.history_version_key(user), PortfolioSnapshot.price_generation_key(user)].each { |k| out[k] = AppConfig.get(k) }
    out
  end

  def record(root)
    ActiveJob::Base.queue_adapter = :test # the backfill's follow-up LedgerJob and the page broadcasts are not run
    Rails.cache = ActiveSupport::Cache::MemoryStore.new
    Faraday::Adapter.lookup_middleware(:net_http_persistent).prepend(ScriptedTracker::Adapter)
    Turbo::StreamsChannel.singleton_class.prepend(Module.new do
      %i[broadcast_remove_to broadcast_update_to broadcast_append_to broadcast_replace_to broadcast_refresh_to].each { |m| define_method(m) { |*, **| nil } }
    end)
    Dir[File.join(root, '*/scenario.json')].sort.each do |path|
      dir = File.dirname(path)
      sc = JSON.parse(File.read(path))
      ActiveRecord::Base.establish_connection(adapter: 'sqlite3', database: File.join(dir, 'production.sqlite3'))
      Rails.cache.clear
      ScriptedTracker.alpaca = sc['alpaca'].transform_values(&:dup)
      ScriptedTracker.market = sc['market'].transform_values(&:dup)
      ScriptedTracker.requests = { 'market' => [], 'alpaca' => [] }
      user = User.find(sc['user_id'])
      ledger = ledger_error = backfill_error = nil
      travel_to(Time.iso8601(sc['at'])) do
        begin
          Tracker::LedgerJob.perform_now(user.id)
          ledger = report(user)
        rescue StandardError => e
          ledger_error = "#{e.class}: #{e.message}"
        end
        begin
          PortfolioSnapshot::BackfillJob.perform_now(user.id)
        rescue StandardError => e
          backfill_error = "#{e.class}: #{e.message}"
        end
      end
      report = { 'ledger' => ledger, 'ledger_error' => ledger_error, 'backfill_error' => backfill_error, 'tables' => tables(user),
                 'requests' => ScriptedTracker.requests }
      File.write(File.join(dir, 'rails.json'), JSON.pretty_generate(report))
      ActiveRecord::Base.connection_pool.disconnect!
    end
  end
end

command, root = ARGV
raise ArgumentError, 'usage: grid <root> | record <root>' unless root

case command
when 'grid' then TrackerParity.grid(root)
when 'record' then TrackerParity.record(root)
else raise ArgumentError, 'usage: grid <root> | record <root>'
end
