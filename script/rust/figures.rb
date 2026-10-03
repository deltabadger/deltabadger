# The Rails half of the figures-parity harness (rust/tests/figures.rs).
#   bin/rails runner script/rust/figures.rb grid <root>     # one install per scenario, built with Rails' own models
#   bin/rails runner script/rust/figures.rb record <root>   # Rails' figures per <root>/<scenario>/ -> rails.json
# Always run with every *_DATABASE_URL pointing at scratch files. Market data is scripted beneath every client, at
# the Faraday adapter they all use (net_http_persistent), so Rails' own parsing, caching and failure handling run.
# A request the scenario did not script raises Harness::Unscripted, and so does any real connection.
require 'json'
require 'fileutils'
require 'tmpdir'
require 'nokogiri'
require 'active_support/testing/time_helpers'

module Harness
  # An Exception, not a StandardError: with_rescue and the candle threads would turn a harness gap into a Failure.
  class Unscripted < Exception; end # rubocop:disable Lint/InheritException
end

Net::HTTP.prepend(Module.new { def connect = raise(Harness::Unscripted, "real connection to #{address}:#{port}") })

module ScriptedMarket
  mattr_accessor :http, :requests, :gaps
  LOCK = Mutex.new # candles are fetched from threads

  NETWORK = {
    # Client.network_failure raises Client::TransientNetworkError for this one (a retry can fix it) ...
    'transient' => ->(host) { Faraday::ConnectionFailed.new(Errno::ECONNREFUSED.new(%(connect(2) for "#{host}" port 443))) },
    # ... and returns a Failure for a certificate error.
    'permanent' => ->(_host) { Faraday::SSLError.new(OpenSSL::SSL::SSLError.new('certificate verify failed')) }
  }.freeze

  # [the key a reply is scripted under, the whole request]. Only the parameters that pick a series are part of the
  # key; the rest (symbols, start, timeframe, limit) are logged and compared with what Rust asks for.
  def self.lines(env)
    path = env.url.path
    query = URI.decode_www_form(env.url.query.to_s).sort
    picks = if path == '/v1beta3/crypto/us/bars' then %w[symbols]
            elsif path.end_with?('/bars') then %w[adjustment]
            elsif path.end_with?('/prices') then %w[coin_ids vs_currencies]
            elsif path.end_with?('/simple/price') then %w[ids vs_currencies]
            else []
            end
    host = [80, 443].include?(env.url.port) ? env.url.host : "#{env.url.host}:#{env.url.port}"
    line = ->(pairs) { ["#{env.method.to_s.upcase} #{host}#{path}", pairs.map { |k, v| "#{k}=#{v}" }.join('&')].reject(&:empty?).join('?') }
    [line.call(query.select { |k, _| picks.include?(k) }), line.call(query)]
  end

  module Adapter
    def call(env)
      raise Harness::Unscripted, "unscripted HTTP call #{env.method.to_s.upcase} #{env.url}" if ScriptedMarket.http.nil?

      key, whole = ScriptedMarket.lines(env)
      LOCK.synchronize { ScriptedMarket.requests << whole }
      reply = ScriptedMarket.http[key]
      if reply.nil?
        # Recorded as well as raised: a candle thread's own rescue is for StandardError, but nothing may hide a gap.
        LOCK.synchronize { ScriptedMarket.gaps << key }
        raise Harness::Unscripted, "unscripted market-data call #{key}"
      end
      if reply['network']
        error = ScriptedMarket::NETWORK.fetch(reply['network']).call(env.url.host)
        actual = "#{error.class}: #{error.message}" # what Client.network_failure reports
        raise Harness::Unscripted, "the script says #{reply['message'].inspect}, Ruby says #{actual.inspect}" unless actual == reply['message']

        raise error
      end

      body = reply['body'].is_a?(String) ? reply['body'] : JSON.generate(reply['body'])
      env.response = Faraday::Response.new
      save_response(env, reply.fetch('status', 200), body, { 'Content-Type' => 'application/json' })
      @app.call(env)
    end
  end
end

module Figures
  extend ActiveSupport::Testing::TimeHelpers
  module_function

  DATA_API = 'http://data-api:3000'.freeze
  T0 = Time.utc(2026, 3, 2, 14, 30, 0) # the first order of most scenarios: a Monday, 14:30 UTC

  # ---- the install ----------------------------------------------------------------------------------------------

  STOCKS = %w[AAA BBB CCC DDD EEE FFF GGG HHH III JJJ KKK LLL].freeze

  # What every scenario starts from: the schema, the owner, the venue, its assets and its listings. Built once, with
  # Rails' own models, and copied per scenario.
  def world(dir)
    { 'production.sqlite3' => 'db/schema.rb', 'production_queue.sqlite3' => 'db/queue_schema.rb' }.each do |file, schema|
      ActiveRecord::Base.establish_connection(adapter: 'sqlite3', database: File.join(dir, file))
      ActiveRecord::Schema.verbose = false
      load Rails.root.join(schema)
    end
    ActiveRecord::Base.establish_connection(adapter: 'sqlite3', database: File.join(dir, 'production.sqlite3'))
    user = User.new(name: 'Owner', email: 'owner@example.com', password: 'correct horse battery staple', admin: true,
                    confirmed_at: Time.current, setup_completed: true)
    user.save!(validate: false)
    alpaca = Exchanges::Alpaca.create!(name: 'Alpaca', maker_fee: '0.15', taker_fee: '0.25')
    ApiKey.new(user:, exchange: alpaca, key: 'k', secret: 's', passphrase: 'paper', status: :correct, key_type: :trading).save!(validate: false)
    assets = { 'USD' => Asset.create!(external_id: 'usd', symbol: 'USD', name: 'US Dollar', category: 'Fiat') }
    STOCKS.each_with_index do |symbol, i|
      assets[symbol] = Asset.create!(external_id: "stock-#{symbol.downcase}", symbol:, name: "#{symbol} Inc.", category: 'Stock',
                                     instrument_type: i.odd? ? 'etf' : 'stock')
    end
    assets['BTC'] = Asset.create!(external_id: 'bitcoin', symbol: 'BTC', name: 'Bitcoin', category: 'Cryptocurrency')
    assets['ETH'] = Asset.create!(external_id: 'ethereum', symbol: 'ETH', name: 'Ethereum', category: 'Cryptocurrency')
    assets['USDT'] = Asset.create!(external_id: 'tether', symbol: 'USDT', name: 'Tether', category: 'Cryptocurrency')
    # A second asset that carries a symbol the venue already lists: its holding takes a suffixed key.
    assets['AAA2'] = Asset.create!(external_id: 'stock-aaa-other', symbol: 'AAA', name: 'Another AAA', category: 'Stock')
    # One with no symbol (its key is its name) and one with neither (its key is its id).
    assets['NAMED'] = Asset.create!(external_id: 'stock-named', symbol: nil, name: 'Nameco', category: 'Stock')
    assets['BARE'] = Asset.create!(external_id: 'stock-bare', symbol: '', name: '', category: 'Stock')
    assets.each_value { |asset| ExchangeAsset.create!(exchange: alpaca, asset:, available: true) }
    ticker = lambda do |base, quote, code, name: base, quote_decimals: 2|
      Ticker.create!(exchange: alpaca, ticker: code, base: name, quote:, base_asset: assets[base], quote_asset: assets[quote],
                     base_decimals: 9, quote_decimals:, price_decimals: 2, minimum_base_size: '0.000000001', minimum_quote_size: '1')
    end
    STOCKS.each { |symbol| ticker.call(symbol, 'USD', symbol) }
    ticker.call('AAA2', 'USD', 'AAA.X', name: 'AAA.X')
    ticker.call('NAMED', 'USD', 'NMD', name: 'NMD')
    ticker.call('BARE', 'USD', 'BRE', name: 'BRE')
    ticker.call('BTC', 'USD', 'BTC/USD', quote_decimals: 4)
    ticker.call('ETH', 'USD', 'ETH/USD', quote_decimals: 4)
    ticker.call('BTC', 'USDT', 'BTC/USDT', quote_decimals: 6)
    ticker.call('ETH', 'USDT', 'ETH/USDT', quote_decimals: 6)
    ActiveRecord::Base.connection_pool.disconnect!
    assets.transform_values(&:id)
  end

  # One scenario's install: the world, then what the scenario says about the owner and the market-data provider.
  def install(sc, template, asset_ids)
    %w[production.sqlite3 production_queue.sqlite3].each { |file| FileUtils.cp(File.join(template, file), File.join(sc['dir'], file)) }
    ActiveRecord::Base.establish_connection(adapter: 'sqlite3', database: File.join(sc['dir'], 'production.sqlite3'))
    user = User.sole
    user.update_columns(hide_balances: sc.fetch('hide_balances', false), display_currency: sc.fetch('display_currency', 'USD'),
                        time_zone: sc.fetch('time_zone', 'UTC'))
    case sc['provider']
    when 'deltabadger'
      AppConfig.market_data_provider = 'deltabadger'
      AppConfig.market_data_url = DATA_API
      AppConfig.market_data_token = 'token'
    when 'coingecko'
      AppConfig.market_data_provider = 'coingecko'
      AppConfig.coingecko_api_key = 'CG-demo-key'
    end
    [user, Exchanges::Alpaca.sole, asset_ids.transform_values { |id| Asset.find(id) }]
  end

  def time(value) = value.is_a?(String) ? Time.iso8601(value) : value

  def bot_for(user, alpaca, assets, spec)
    quote = assets.fetch(spec.fetch('quote', 'USD'))
    members = spec.fetch('members')
    bot = case spec.fetch('type')
          when 'basket'
            weights = spec['weights'] || members.map { 1.0 / members.size }
            user.bots.new(type: 'Bots::DcaMultiAsset', exchange: alpaca, label: spec['label'], settings: {
              'quote_asset_id' => quote.id, 'quote_amount' => 100.0, 'interval' => 'week', 'weighting' => 'manual',
              'allocations' => members.each_with_index.to_h { |symbol, i| [assets.fetch(symbol).id.to_s, weights[i]] }
            }.merge(spec.fetch('settings', {})))
          when 'index'
            user.bots.new(type: 'Bots::DcaIndex', exchange: alpaca, label: spec['label'], settings: {
              'quote_asset_id' => quote.id, 'quote_amount' => 100.0, 'interval' => 'week', 'num_coins' => members.size,
              'allocation_flattening' => 0.0, 'index_type' => 'category', 'index_category_id' => 'nasdaq-100',
              'index_name' => 'ND100', 'limit_ordered' => false
            }.merge(spec.fetch('settings', {})))
          when 'single' # a pair bot: Rails computes its figures, this library does not
            user.bots.new(type: 'Bots::DcaSingleAsset', exchange: alpaca, label: spec['label'], settings: {
              'base_asset_id' => assets.fetch(members.first).id, 'quote_asset_id' => quote.id, 'quote_amount' => 100.0, 'interval' => 'week'
            })
          end
    bot.set_missed_quote_amount
    bot.save!(validate: spec['type'] == 'basket')
    if spec['type'] == 'index' # the composition an index derives from market data, written as refresh_composition writes it
      members.each do |symbol|
        asset = assets.fetch(symbol)
        BotIndexAsset.create!(bot:, asset:, ticker: alpaca.tickers.find_by!(base_asset: asset, quote_asset: quote),
                              target_allocation: (1.0 / members.size).round(6), in_index: true, entered_at: T0)
      end
    end
    Array(spec['exited']).each do |symbol| # a holding that left the composition keeps its row
      asset = assets.fetch(symbol)
      BotIndexAsset.create!(bot:, asset:, ticker: alpaca.tickers.find_by!(base_asset: asset, quote_asset: quote),
                            target_allocation: 0, in_index: false, entered_at: T0, exited_at: T0 + 1.day)
    end
    bot.update_columns(status: Bot.statuses[spec.fetch('status', 'scheduled')], started_at: T0)
    rows = spec.fetch('orders', []).each_with_index.map do |order, i|
      symbol = order.fetch('sym')
      asset = assets.fetch(order.fetch('asset', symbol)) unless order['asset'] == false
      { 'bot_id' => bot.id, 'exchange_id' => alpaca.id, 'base_asset_id' => asset&.id, 'quote_asset_id' => quote.id,
        'base' => order.fetch('base', symbol), 'quote' => quote.symbol, 'side' => order.fetch('side', 'buy'),
        'transaction_type' => order.fetch('type', 'REGULAR'), 'status' => order.fetch('status', 'submitted'),
        'external_status' => order.key?('ext') ? order['ext'] : 'closed', 'external_id' => "#{spec['label'] || bot.id}-#{i}",
        'order_type' => order.fetch('order_type', 'market_order'), 'price' => order['price'], 'amount' => order['amount'],
        'quote_amount' => order['quote_amount'], 'amount_exec' => order['amount_exec'], 'quote_amount_exec' => order['quote_amount_exec'],
        'bot_interval' => 'week', 'bot_quote_amount' => 100.0, 'error_messages' => [],
        'created_at' => time(order.fetch('at')), 'updated_at' => time(order.fetch('at')) }
    end
    # insert_all, in slices: a row can hold what an old row holds, and a history of twenty thousand takes seconds.
    rows.each_slice(500) { |slice| Transaction.insert_all!(slice) }
    bot
  end

  def build(sc, template, asset_ids)
    user, alpaca, assets = install(sc, template, asset_ids)
    bots = sc.fetch('bots').map { |spec| bot_for(user, alpaca, assets, spec) }
    sc.fetch('splits', []).each_with_index do |split, i|
      AccountTransaction.insert!({
        'user_id' => user.id, 'exchange_id' => alpaca.id, 'entry_type' => 'adjustment', 'base_currency' => split.fetch('sym'),
        'base_asset_id' => assets[split['sym']]&.id, 'base_amount' => split.fetch('amount', '0'), 'tx_id' => "split-#{i}",
        'raw_data' => { 'corporate_action' => 'split', 'split_ratio' => split['ratio'] }.compact,
        'transacted_at' => time(split.fetch('at')), 'created_at' => time(split.fetch('at')), 'updated_at' => time(split.fetch('at'))
      })
    end
    Array(sc['delist']).each { |code| alpaca.tickers.find_by!(ticker: code).update_columns(available: false) }
    [user, bots]
  end

  # ---- market data, as the venues and the two rate providers answer --------------------------------------------

  ALPACA = 'GET data.alpaca.markets'.freeze
  HOSTED = 'GET data-api:3000/api/v1'.freeze
  COINGECKO = 'GET api.coingecko.com/api/v3'.freeze
  REFUSED = { 'network' => 'transient', 'message' => 'Faraday::ConnectionFailed: Connection refused - connect(2) for "data.alpaca.markets" port 443' }.freeze
  CERTIFICATE = { 'network' => 'permanent', 'message' => 'Faraday::SSLError: certificate verify failed' }.freeze
  SERVER_ERROR = { 'status' => 500, 'body' => { 'code' => 50_010_000, 'message' => 'internal server error' } }.freeze
  STOCK_PRICES = "#{ALPACA}/v2/stocks/snapshots".freeze
  CRYPTO_PRICES = "#{ALPACA}/v1beta3/crypto/us/latest/trades".freeze

  def ok(body) = { 'status' => 200, 'body' => body }

  # [[time, open], ...] as Alpaca bars. Only the open is read by the chart; the rest is filled in plausibly.
  def bars(rows) = rows.map { |at, open| { 't' => time(at).utc.iso8601, 'o' => open, 'h' => open, 'l' => open, 'c' => open, 'v' => 1000 } }

  # Opens from `from`, `count` of them, `step` seconds apart: `first` plus `drift` each.
  def every(from, step, count, first, drift) = Array.new(count) { |i| [time(from) + (i * step), (first + (drift * i)).round(4)] }

  def daily(from, count, first, drift) = every(from, 86_400, count, first, drift)

  def market(prices: {}, crypto: {}, stock_bars: {}, restated_bars: nil, crypto_bars: {}, extra: {})
    script = {}
    script[STOCK_PRICES] = ok(prices.transform_values { |p| p.nil? ? {} : { 'latestTrade' => { 'p' => p } } }) if prices
    script[CRYPTO_PRICES] = ok('trades' => crypto.transform_values { |p| { 'p' => p } }) if crypto
    stock_bars.each { |symbol, rows| script["#{ALPACA}/v2/stocks/#{symbol}/bars"] = ok('bars' => bars(rows), 'symbol' => symbol) }
    # A stock's history is asked for twice: as traded, and restated onto today's share basis (the price overlay).
    (restated_bars || stock_bars).each { |symbol, rows| script["#{ALPACA}/v2/stocks/#{symbol}/bars?adjustment=split"] = ok('bars' => bars(rows), 'symbol' => symbol) }
    crypto_bars.each { |pair, rows| script["#{ALPACA}/v1beta3/crypto/us/bars?symbols=#{pair}"] = ok('bars' => { pair => bars(rows) }) }
    script.merge(extra)
  end

  # The hosted market-data API's answers: BTC-based rates for fiat pairs, a coin's price for the rest.
  def hosted(rates: nil, prices: {})
    script = {}
    script["#{HOSTED}/exchange_rates"] = ok('data' => rates.transform_values { |value| { 'name' => 'x', 'unit' => 'x', 'value' => value, 'type' => 'fiat' } }) if rates
    prices.each { |(coin, currency), price| script["#{HOSTED}/prices?coin_ids=#{coin}&vs_currencies=#{currency}"] = ok('data' => { coin => { currency => price } }) }
    script
  end

  def coingecko(rates: nil, prices: {})
    script = {}
    script["#{COINGECKO}/exchange_rates"] = ok('rates' => rates.transform_values { |value| { 'name' => 'x', 'unit' => 'x', 'value' => value, 'type' => 'fiat' } }) if rates
    prices.each { |(coin, currency), price| script["#{COINGECKO}/simple/price?ids=#{coin}&vs_currencies=#{currency}"] = ok(coin => { currency => price }) }
    script
  end

  # ---- orders ----------------------------------------------------------------------------------------------------

  def buy(sym, at, price, amount, **rest)
    { 'sym' => sym, 'at' => at, 'price' => price, 'amount' => amount, 'amount_exec' => amount,
      'quote_amount_exec' => (BigDecimal(price) * BigDecimal(amount)).to_s }.merge(rest.stringify_keys)
  end

  def sell(sym, at, price, amount, **rest) = buy(sym, at, price, amount, side: 'sell', **rest)

  def basket(members, orders, **rest) = { 'type' => 'basket', 'members' => members, 'orders' => orders }.merge(rest.stringify_keys)
  def index(members, orders, **rest) = { 'type' => 'index', 'members' => members, 'orders' => orders }.merge(rest.stringify_keys)

  # One asset bought every ten minutes for 139 days, sold whole, and bought again; beside it a holding with no price
  # and no candles, so every one of the 20,000 points keeps its fill mark and every candle time is looked up and dropped.
  LONG = 20_000

  def long_history
    buys = Array.new(LONG) { |i| buy('AAA', T0 + (i * 600), (100 + ((i % 700) * 0.01)).round(2).to_s, '0.01') }
    [buy('BBB', T0 + 1, '55.21', '0.5')] + buys +
      [sell('AAA', T0 + (LONG * 600), '104', (LONG * 0.01).round(2).to_s, type: 'LIQUIDATION')] +
      Array.new(3) { |i| buy('AAA', T0 + ((LONG + 1 + i) * 600), '104.5', '0.01') }
  end

  # Three units bought for 100; then, `pairs` times, one of them sold and one bought back. Rails takes the sold
  # third of the holding's cost off it, and a third is 32 digits: priced, the cost gains 32 digits with every sale.
  # Not priced (a REBALANCE sale the venue reported no proceeds for), the estimate of the proceeds is the cost itself,
  # the buy divides by it, and the cost's digits double with every sale.
  def pairs(count, priced:)
    sale = lambda do |at|
      priced ? sell('AAA', at, '34', '1') : { 'sym' => 'AAA', 'at' => at, 'price' => '0', 'amount' => '1', 'amount_exec' => '1', 'side' => 'sell', 'type' => 'REBALANCE' }
    end
    [{ 'sym' => 'AAA', 'at' => T0, 'price' => '33.33', 'amount' => '3', 'amount_exec' => '3', 'quote_amount_exec' => '100' }] +
      Array.new(count) { |i| [sale.call(T0 + (((2 * i) + 1) * 600)), buy('AAA', T0 + (((2 * i) + 2) * 600), '33', '1')] }.flatten(1)
  end

  def scenario(name, at, bots, script, **rest) = { 'name' => name, 'at' => at, 'bots' => Array.wrap(bots), 'script' => script }.merge(rest.stringify_keys)

  D = 86_400
  BARS_START = T0 - (2 * D) - 37_800 # 04:00 UTC two days before the first order: where Alpaca opens a daily bar

  WEEKLY = [buy('AAA', T0, '101.37', '0.591891092'), buy('BBB', T0 + 1, '55.21', '0.724506430'),
            buy('AAA', T0 + (7 * D), '103.11', '0.581902822'), buy('BBB', T0 + (7 * D) + 1, '54.02', '0.740466494')].freeze
  AAA_BARS = daily(BARS_START, 14, 100.0, 0.41).freeze
  BBB_BARS = daily(BARS_START, 14, 56.0, -0.17).freeze
  TWO = market(prices: { 'AAA' => 104.52, 'BBB' => 53.9 }, stock_bars: { 'AAA' => AAA_BARS, 'BBB' => BBB_BARS }).freeze
  NOW = T0 + (10 * D) + 0.25
  ACCOUNT_MARKET = market(prices: { 'AAA' => 104.52, 'BBB' => 53.9, 'DDD' => 81.5 },
                          stock_bars: { 'AAA' => AAA_BARS, 'BBB' => BBB_BARS, 'DDD' => daily(BARS_START, 14, 79.0, 0.2) }).freeze

  # What the orders table can hold for one order, and how the walk, the tax lots and the chart marks each read it.
  def row_readings
    [buy('AAA', T0, '100', '1'),
     { 'sym' => 'AAA', 'at' => T0 + 60, 'price' => '101', 'amount' => '1', 'amount_exec' => '0.4', 'ext' => 'open' }, # executed, proceeds not reported yet
     { 'sym' => 'AAA', 'at' => T0 + 120, 'price' => '100.25', 'amount' => '1', 'amount_exec' => '0.4', 'quote_amount_exec' => '40.1', 'ext' => 'open' },
     { 'sym' => 'AAA', 'at' => T0 + 180, 'price' => '99', 'amount' => '1', 'amount_exec' => '0', 'quote_amount_exec' => '0', 'ext' => 'cancelled' },
     { 'sym' => 'AAA', 'at' => T0 + 240, 'price' => '99', 'amount' => '1', 'ext' => 'cancelled' },
     { 'sym' => 'AAA', 'at' => T0 + 300, 'price' => '100', 'amount' => '0.5' }, # a closed row never backfilled
     { 'sym' => 'AAA', 'at' => T0 + 360, 'price' => '102', 'amount' => '0.3', 'amount_exec' => '0.3', 'quote_amount_exec' => '0' }, # Alpaca's zero
     { 'sym' => 'AAA', 'at' => T0 + 420, 'amount' => '0.2', 'amount_exec' => '0.2', 'quote_amount_exec' => '21' }, # no order price
     { 'sym' => 'AAA', 'at' => T0 + 480, 'price' => '0', 'amount' => '0.1', 'amount_exec' => '0.1' }, # a lot of unknown cost
     buy('AAA', T0 + 540, '98', '1', status: 'failed'), buy('AAA', T0 + 600, '98', '1', status: 'skipped'),
     buy('AAA', T0 + 660, '103.5', '0.25', ext: 'unknown'), buy('AAA', T0 + 720, '104', '0.125', ext: 'abandoned'),
     { 'sym' => 'AAA', 'at' => T0 + 780, 'price' => '104', 'amount' => '0.5', 'amount_exec' => '0.25', 'quote_amount_exec' => '26.01', 'ext' => 'cancelled' },
     buy('AAA', T0 + 840, '105', '0.2', side: nil, type: 'ODD')] # no side recorded: read as a buy
  end

  def slices
    Array.new(12) { |i| buy('AAA', T0 + (i * 300) + (i.odd? ? 0.123456 : 0), (100 + (i * 0.07)).round(2).to_s, format('%.9f', 10 / (100 + (i * 0.07)))) } +
      [buy('BBB', T0 + 600, '55.5', '0.180180180'), buy('BBB', T0 + 600, '55.6', '0.179856115')] # one moment, two orders: by id
  end

  TEN = %w[AAA BBB CCC DDD EEE FFF GGG HHH III JJJ].freeze

  def week_of(symbols, at, drift)
    symbols.each_with_index.map { |symbol, i| buy(symbol, at, (50 + (i * 13.37) + drift).round(2).to_s, format('%.9f', 10 / (50 + (i * 13.37) + drift))) }
  end

  def index_prices(extra = {}) = (STOCKS + %w[AAA.X NMD BRE]).each_with_index.to_h { |symbol, i| [symbol, (51.5 + (i * 13.4)).round(2)] }.merge(extra)

  def index_market(symbols, extra_prices: {}, **rest)
    market(prices: index_prices(extra_prices), crypto: { 'BTC/USD' => 64_000.5, 'ETH/USD' => 3100.25 },
           stock_bars: symbols.each_with_index.to_h { |symbol, i| [symbol, daily(BARS_START, 25, 50 + (i * 13.3), 0.11 * (i - 4))] }, **rest)
  end

  SPLIT_ORDERS = [buy('AAA', T0, '400', '0.25'), buy('BBB', T0 + 1, '55.21', '0.724506430'),
                  buy('AAA', T0 + (7 * D), '103.11', '0.581902822'), buy('BBB', T0 + (7 * D) + 1, '54.02', '0.740466494')].freeze
  SPLIT_AT = T0 + (3 * D) - 52_200 # midnight UTC on the fourth day, ahead of that day's first bar
  # As traded: four times the price before the split. Restated: one basis end to end.
  RAW_AAA = (daily(BARS_START, 5, 400.0, 1.64) + daily(BARS_START + (5 * D), 9, 102.05, 0.41)).freeze

  def split_market(prices = { 'AAA' => 104.52, 'BBB' => 53.9 })
    market(prices:, stock_bars: { 'AAA' => RAW_AAA, 'BBB' => BBB_BARS }, restated_bars: { 'AAA' => AAA_BARS, 'BBB' => BBB_BARS })
  end

  def swaps
    WEEKLY + [sell('AAA', T0 + (8 * D), '104', '0.4', type: 'REBALANCE'),
              buy('BBB', T0 + (8 * D) + 5, '54.1', '0.384473198', type: 'REBALANCE'), # half of the proceeds
              buy('BBB', T0 + (8 * D) + 9, '54.2', '0.380073801', type: 'REBALANCE'), # most of the rest: a dust remainder stays in flight
              buy('AAA', T0 + (9 * D), '104.4', '0.957854406')] # a contribution drains the remainder
  end

  def liquidations
    week_of(TEN, T0, 0) + week_of(TEN, T0 + (7 * D), 1.5) +
      [sell('JJJ', T0 + (8 * D), '175.5', '0.116511208', type: 'LIQUIDATION'), # above cost
       sell('III', T0 + (8 * D) + 1, '150', '0.050000000', type: 'LIQUIDATION'), # part of a holding, below cost
       buy('AAA', T0 + (8 * D) + 60, '52', '0.3', type: 'REDEPLOY'),
       buy('BBB', T0 + (8 * D) + 61, '65', '0.3', type: 'REDEPLOY'), # more than the proceeds left: the rest is a contribution
       buy('CCC', T0 + (9 * D), '78', '0.128205128')]
  end

  def dca_out
    [buy('AAA', T0, '100', '1'), buy('AAA', T0 + D, '110', '1'),
     sell('AAA', T0 + (2 * D), '120', '0.5'), sell('AAA', T0 + (3 * D), '90', '0.75'),
     sell('AAA', T0 + (4 * D), '95', '1.5'), # more than the bot bought: the excess is valued at its own price
     buy('AAA', T0 + (5 * D), '97', '0.2')]
  end

  # Sales of coins the ledger never held, one per kind of sale.
  def never_bought
    [sell('AAA', T0, '100', '0.5'), sell('BBB', T0 + 1, '55', '1', type: 'REBALANCE'), sell('CCC', T0 + 2, '70', '1', type: 'LIQUIDATION'),
     buy('BBB', T0 + 3, '55.5', '0.5', type: 'REBALANCE'), buy('AAA', T0 + D, '101', '0.25')]
  end

  # Sales the venue executed and did not price, one per kind of sale, and one with nothing held behind it.
  def unpriced
    unpriced = ->(sym, at, type, amount) { { 'sym' => sym, 'at' => at, 'price' => '0', 'amount' => amount, 'amount_exec' => amount, 'side' => 'sell', 'type' => type } }
    [buy('AAA', T0, '100', '1'), buy('BBB', T0 + 1, '50', '2'), buy('CCC', T0 + 2, '70', '1'),
     unpriced.call('AAA', T0 + D, 'REGULAR', '0.4'), unpriced.call('BBB', T0 + D + 1, 'REBALANCE', '0.5'),
     unpriced.call('CCC', T0 + D + 2, 'LIQUIDATION', '0.25'), unpriced.call('DDD', T0 + D + 3, 'REGULAR', '1'),
     unpriced.call('AAA', T0 + D + 4, 'REGULAR', '5'), # more than is held: only what the ledger holds is booked
     buy('DDD', T0 + (2 * D), '80', '0.5', type: 'REBALANCE'),
     { 'sym' => 'CCC', 'at' => T0 + (2 * D) + 1, 'price' => '71', 'amount' => '0.5', 'amount_exec' => '0.5', 'side' => 'sell', 'type' => 'LIQUIDATION', 'ext' => 'open' }]
  end

  def lots
    [buy('AAA', T0, '100', '1'), buy('AAA', T0 + D, '120', '1'), buy('AAA', T0 + (2 * D), '90', '1'),
     sell('AAA', T0 + (3 * D), '110', '1.5'), # the first lot gains, half of the second loses
     sell('AAA', T0 + (4 * D), '125', '0.25'), # no lot loses
     { 'sym' => 'BBB', 'at' => T0 + 5, 'price' => '0', 'amount' => '1', 'amount_exec' => '1' }, # cost unknown
     buy('BBB', T0 + 6, '50', '1'),
     sell('BBB', T0 + (3 * D) + 1, '60', '1.5'), # consumes the unknown lot: unknown, unless another lot already shows a loss
     sell('BBB', T0 + (4 * D) + 1, '40', '0.25'),
     buy('CCC', T0 + 7, '70', '3'), sell('CCC', T0 + (5 * D), '70', '3'), # break-even on a lot of three
     buy('DDD', T0 + 8, '80', '1')]
  end

  # Rows recorded before orders stored their asset, beside rows that did; two assets called AAA; unnamed assets.
  def old_rows
    [buy('AAA', T0, '100', '1', asset: false), buy('AAA', T0 + 1, '101', '1'), buy('AAA', T0 + 2, '20', '2', asset: 'AAA2', base: 'AAA.X'),
     buy('ZZZ', T0 + 3, '5', '4', asset: false), buy('NMD', T0 + 4, '30', '1', asset: 'NAMED'), buy('BRE', T0 + 5, '40', '1', asset: 'BARE'),
     buy('aaa', T0 + 6, '99', '0.5', asset: false),
     sell('AAA', T0 + D, '95', '0.5', type: 'LIQUIDATION'), # the lots of the same name recorded without the asset may be the ones sold
     buy('BBB', T0 + D + 1, '55', '1', asset: false), sell('BBB', T0 + D + 2, '56', '1', asset: false),
     buy('BBB', T0 + D + 3, '55', '1'), sell('BBB', T0 + D + 4, '54', '0.5', type: 'LIQUIDATION')]
  end

  def crypto_orders(quote_step = 1)
    [buy('BTC', T0, '64000.12', '0.000937498'), buy('ETH', T0 + quote_step, '3100.55', '0.012901001'),
     buy('BTC', T0 + (7 * D), '65010.5', '0.000922928'), buy('ETH', T0 + (7 * D) + quote_step, '3055.01', '0.013093246')]
  end

  def crypto_market(quote, extra = {})
    market(prices: nil, crypto: { "BTC/#{quote}" => 65_500.25, "ETH/#{quote}" => 3020 },
           crypto_bars: { "BTC/#{quote}" => every(T0 - 3600, 3600, 245, 64_000, 6.5), "ETH/#{quote}" => every(T0 - 3600, 3600, 245, 3100, -0.31) }, extra:)
  end

  # Three bots in two quote currencies, shown in euros: what the list's totals are made of.
  def account(provider, fx, **rest)
    scenario("account_#{provider}", NOW,
             [basket(%w[AAA BBB], WEEKLY, label: 'usd'), basket(%w[BTC ETH], crypto_orders, quote: 'USDT', label: 'usdt'),
              basket(%w[CCC], [], label: 'empty'), basket(%w[DDD], [buy('DDD', T0, '80', '1')], label: 'gone', status: 'deleted'),
              basket(%w[BBB], [buy('BBB', T0 + D, '55', '2')], label: 'shelved', status: 'archived')],
             ACCOUNT_MARKET.merge(crypto_market('USDT')).merge(fx), provider:, display_currency: 'EUR', time_zone: 'Warsaw', **rest)
  end

  RATES = { 'btc' => 1, 'usd' => 64_123.456, 'eur' => 55_210.987, 'pln' => 236_011.5 }.freeze

  # A bot `age` seconds old, charted on the candles Rails picks for that age.
  def aged(name, age, step, timeframe_count)
    first = T0
    now = first + age
    scenario(name, now, basket(%w[AAA], [buy('AAA', first, '100.5', '0.5'), buy('AAA', first + (age / 2), '101.25', '0.5')]),
             market(prices: { 'AAA' => 102.75 }, stock_bars: { 'AAA' => every(first - step, step, timeframe_count, 100.0, 0.013) }))
  end

  # ---- seeded histories: every kind of order in an arbitrary order, with splits ------------------------------

  def random_history(seed)
    rng = Random.new(seed)
    symbols = %w[AAA BBB CCC]
    at = T0
    price = { 'AAA' => 100.0, 'BBB' => 55.0, 'CCC' => 250.0 }
    orders = []
    splits = []
    28.times do
      at += rng.rand(600..(3 * D)) + (rng.rand < 0.3 ? rng.rand(1..999_999) / 1_000_000r : 0)
      sym = symbols.sample(random: rng)
      price[sym] = (price[sym] * (0.93 + (rng.rand * 0.14))).round(2)
      px = format('%.2f', price[sym])
      amount = format('%.9f', rng.rand(5.0..60.0) / price[sym])
      roll = rng.rand
      orders <<
        if roll < 0.50 then buy(sym, at, px, amount)
        elsif roll < 0.58 then sell(sym, at, px, amount, type: 'REBALANCE')
        elsif roll < 0.68 then buy(sym, at, px, amount, type: 'REBALANCE')
        elsif roll < 0.74 then sell(sym, at, px, amount, type: 'LIQUIDATION')
        elsif roll < 0.79 then buy(sym, at, px, amount, type: 'REDEPLOY')
        elsif roll < 0.85 then sell(sym, at, px, amount)
        elsif roll < 0.89 then { 'sym' => sym, 'at' => at, 'price' => px, 'amount' => amount, 'amount_exec' => amount, 'side' => 'sell', 'type' => %w[REGULAR REBALANCE LIQUIDATION].sample(random: rng) }
        elsif roll < 0.93 then { 'sym' => sym, 'at' => at, 'price' => px, 'amount' => amount, 'amount_exec' => format('%.9f', amount.to_f / 3), 'ext' => 'open' }
        elsif roll < 0.96 then { 'sym' => sym, 'at' => at, 'price' => px, 'amount' => amount, 'side' => %w[buy sell].sample(random: rng) }
        else { 'sym' => sym, 'at' => at, 'price' => px, 'amount' => amount, 'amount_exec' => '0', 'quote_amount_exec' => '0', 'ext' => 'cancelled' }
        end
      next unless rng.rand < 0.08

      ratio = ['2:1', '3:2', '1:10', '10:1', '4:1', '1.5:1'].sample(random: rng)
      splits << { 'sym' => sym, 'at' => at + rng.rand(60..D), 'ratio' => ratio }
      new_count, old_count = ratio.split(':').map(&:to_f)
      price[sym] = (price[sym] * old_count / new_count).round(2)
    end
    now = at + (4 * D) + 0.5
    grid = symbols.to_h { |sym| [sym, daily(T0 - (3 * D), ((now - T0) / D).to_i + 4, price[sym] * 0.9, price[sym] * 0.002)] }
    scenario("random_#{seed}", now, basket(symbols, orders), market(prices: price.transform_values { |p| (p * 1.013).round(2) }, stock_bars: grid), 'splits' => splits)
  end

  # ---- the grid --------------------------------------------------------------------------------------------------

  def scenarios
    two = %w[AAA BBB]
    list = [
      # The walk over the orders: Bot::Composition::Measurable#metrics.
      scenario('no_orders', T0 + (10 * D), basket(two, [], label: 'empty'), market),
      # Shown in London, which in March is at no offset from UTC and is not UTC: the chart's times end in +00:00.
      scenario('float_quote_id', NOW, basket(two, WEEKLY, settings: { 'quote_asset_id' => 1.0 }), TWO),
      scenario('fractional_quote_id', NOW, basket(two, WEEKLY, settings: { 'quote_asset_id' => 1.9 }), TWO),
      scenario('basket_buys', NOW, basket(two, WEEKLY, weights: [0.6, 0.4]), TWO, time_zone: 'London'),
      scenario('row_readings', NOW, basket(%w[AAA], row_readings, settings: { 'quote_amount_limited' => true, 'quote_amount_limit' => 1000 }),
               market(prices: { 'AAA' => 104.52 }, stock_bars: { 'AAA' => AAA_BARS })),
      scenario('smart_slices', T0 + 7200.5, basket(two, slices, settings: { 'smart_intervaled' => true, 'smart_interval_quote_amount' => 10.0 }),
               market(prices: { 'AAA' => 100.9, 'BBB' => 55.4 },
                      stock_bars: { 'AAA' => every(T0 - 60, 60, 110, 100.0, 0.011), 'BBB' => every(T0 - 60, 60, 110, 55.5, -0.003) })),
      scenario('index_rotation', T0 + (16 * D) + 0.75,
               index(TEN.first(9) + %w[KKK], week_of(TEN, T0, 0) + week_of(TEN.first(9) + %w[KKK], T0 + (7 * D), 1.5) +
                                               week_of(TEN.first(9) + %w[KKK], T0 + (14 * D), -0.8) + [buy('LLL', T0 + 30, '200', '0.05')],
                     exited: %w[JJJ LLL]),
               index_market(TEN + %w[KKK]), delist: %w[LLL]),
      scenario('swaps', NOW, basket(two, swaps), TWO),
      scenario('liquidations', NOW, index(TEN.first(8), liquidations, exited: %w[III JJJ]), index_market(TEN)),
      scenario('dca_out', NOW, basket(%w[AAA], dca_out, settings: { 'direction' => 'selling' }), market(prices: { 'AAA' => 104.52 }, stock_bars: { 'AAA' => AAA_BARS })),
      scenario('never_bought', NOW, basket(%w[AAA BBB CCC], never_bought), index_market(%w[AAA BBB CCC])),
      scenario('unpriced_sales', NOW, basket(%w[AAA BBB CCC DDD], unpriced), index_market(%w[AAA BBB CCC DDD])),
      scenario('tax_lots', NOW, basket(%w[AAA BBB CCC DDD], lots), index_market(%w[AAA BBB CCC DDD], extra_prices: { 'AAA' => 80.0, 'DDD' => 79.99 })),
      # Two holdings on one ticker read one candle series twice. Hourly bars up to now keep Rails' candle cache fresh, so it
      # has no tail to ask the venue for on the second read (the Rust side has no cache and asks once per holding).
      scenario('old_rows', NOW, basket(%w[AAA BBB AAA2 NAMED BARE], old_rows),
               index_market([]).merge(market(prices: nil, crypto: nil, stock_bars: %w[AAA BBB AAA.X NMD BRE].each_with_index.to_h { |symbol, i| [symbol, every(T0 - 3600, 3600, 245, 20 + (i * 21.5), 0.01 * (i - 2))] }))),
      scenario('sold_out', NOW, basket(%w[AAA], [buy('AAA', T0, '100', '1'), sell('AAA', T0 + D, '110', '1')]),
               market(prices: { 'AAA' => 104.52 }, stock_bars: { 'AAA' => AAA_BARS })),
      scenario('crypto_basket', NOW, basket(%w[BTC ETH], crypto_orders), crypto_market('USD')),
      # A fill the venue reported below zero: what went in rounds to a zero that keeps its sign ("-0.0").
      scenario('negative_fill', NOW, basket(%w[AAA], [{ 'sym' => 'AAA', 'at' => T0, 'price' => '100', 'amount' => '0.00001', 'amount_exec' => '0.00001', 'quote_amount_exec' => '-0.001' },
                                                   buy('AAA', T0 + D, '101', '0.5')]),
               market(prices: { 'AAA' => 104.52 }, stock_bars: { 'AAA' => AAA_BARS })),
      # 700 priced sales, each followed by a buy: a cost of 22,400 digits at the end, and one of its own at every point.
      scenario('priced_pairs', NOW, basket(%w[AAA], pairs(700, priced: true)),
               market(prices: { 'AAA' => 34.5 }, stock_bars: { 'AAA' => daily(BARS_START, 14, 33.0, 0.1) })),
      # Ten sales with no proceeds reported, each followed by a buy: a cost of 74,672 digits. Every pair more takes
      # Rails four times as long (rust/examples/figures_limits.rs has the twelfth, which this library refuses).
      scenario('unpriced_rebalances', NOW, basket(%w[AAA], pairs(11, priced: false)),
               market(prices: { 'AAA' => 34.5 }, stock_bars: { 'AAA' => daily(BARS_START, 14, 33.0, 0.1) })),
      scenario('long_history', T0 + ((LONG + 10) * 600) + 0.5, basket(two, long_history),
               market(prices: { 'AAA' => 104.52 }, stock_bars: { 'AAA' => daily(BARS_START + (30 * D), 70, 100.0, 0.05) })
                 .merge("#{ALPACA}/v2/stocks/BBB/bars" => SERVER_ERROR, "#{ALPACA}/v2/stocks/BBB/bars?adjustment=split" => SERVER_ERROR)),

      # Restatements: Bot::Restatable.
      scenario('split', NOW, basket(two, SPLIT_ORDERS), split_market,
               splits: [{ 'sym' => 'AAA', 'at' => SPLIT_AT, 'ratio' => '4:1' },
                        { 'sym' => 'BBB', 'at' => T0 - D, 'ratio' => '2:1' }, # before the first fill: moves nothing
                        { 'sym' => 'AAA', 'at' => T0 + (30 * D), 'ratio' => '2:1' }, # dated ahead: not in effect
                        { 'sym' => 'CCC', 'at' => T0 + D, 'ratio' => '3:1' }]), # never traded
      scenario('split_after_the_bar', NOW, basket(two, SPLIT_ORDERS), split_market, # booked five hours late: one bar is read on the old count
               splits: [{ 'sym' => 'AAA', 'at' => SPLIT_AT + 18_000, 'ratio' => '4:1' }]),
      scenario('split_fresh', T0 + (4 * D), basket(two, SPLIT_ORDERS.first(2)), split_market,
               splits: [{ 'sym' => 'AAA', 'at' => SPLIT_AT, 'ratio' => '4:1' }]), # inside the two-day quarantine
      scenario('split_quarantine_over', SPLIT_AT + (2 * D), basket(two, SPLIT_ORDERS.first(2)), split_market, # two days to the instant: trusted again
               splits: [{ 'sym' => 'AAA', 'at' => SPLIT_AT, 'ratio' => '4:1' }]),
      scenario('split_at_order', NOW, basket(two, SPLIT_ORDERS + [buy('AAA', SPLIT_AT, '100.1', '0.5')]), split_market,
               splits: [{ 'sym' => 'AAA', 'at' => SPLIT_AT, 'ratio' => '4:1' }]),
      # A fill at the split's own instant, where the candles do not reach: it is on the new count, so it fills in a grid
      # whose candles begin after the split and stays out of one whose candles end before it.
      scenario('split_fill_before_the_candles', NOW, basket(two, SPLIT_ORDERS + [buy('AAA', SPLIT_AT, '100.1', '0.5')]),
               market(prices: { 'AAA' => 104.52, 'BBB' => 53.9 }, stock_bars: { 'AAA' => RAW_AAA.last(9), 'BBB' => BBB_BARS },
                      restated_bars: { 'AAA' => AAA_BARS, 'BBB' => BBB_BARS }),
               splits: [{ 'sym' => 'AAA', 'at' => SPLIT_AT, 'ratio' => '4:1' }]),
      scenario('split_fill_past_the_candles', NOW, basket(two, SPLIT_ORDERS + [buy('AAA', SPLIT_AT, '100.1', '0.5')]),
               market(prices: { 'BBB' => 53.9 }, stock_bars: { 'AAA' => RAW_AAA.first(5), 'BBB' => BBB_BARS },
                      restated_bars: { 'AAA' => AAA_BARS, 'BBB' => BBB_BARS }),
               splits: [{ 'sym' => 'AAA', 'at' => SPLIT_AT, 'ratio' => '4:1' }]),
      scenario('split_two_reports', NOW, basket(two, SPLIT_ORDERS), split_market,
               splits: [{ 'sym' => 'AAA', 'at' => SPLIT_AT + 7200, 'ratio' => nil }, # the add leg alone names no factor
                        { 'sym' => 'AAA', 'at' => SPLIT_AT, 'ratio' => '4:1' }, { 'sym' => 'AAA', 'at' => SPLIT_AT + 60, 'ratio' => '8:2' }]),
      scenario('split_unsized', NOW, basket(two, SPLIT_ORDERS), split_market, splits: [{ 'sym' => 'AAA', 'at' => SPLIT_AT, 'ratio' => nil }]),
      scenario('split_conflict', NOW, basket(two, SPLIT_ORDERS), split_market,
               splits: [{ 'sym' => 'AAA', 'at' => SPLIT_AT, 'ratio' => '4:1' }, { 'sym' => 'AAA', 'at' => SPLIT_AT + 60, 'ratio' => '2:1' }]),
      scenario('split_unreadable', NOW, basket(two, SPLIT_ORDERS), split_market,
               splits: [{ 'sym' => 'AAA', 'at' => SPLIT_AT, 'ratio' => '4:0' }, { 'sym' => 'BBB', 'at' => SPLIT_AT, 'ratio' => 'four for one' }]),
      scenario('split_reverse', NOW, basket(two, WEEKLY), TWO,
               splits: [{ 'sym' => 'AAA', 'at' => SPLIT_AT, 'ratio' => '1:10' }, { 'sym' => 'BBB', 'at' => SPLIT_AT + (5 * D), 'ratio' => '3:2' }]),
      scenario('split_sold_out', NOW, basket(two, [buy('AAA', T0, '400', '0.25'), sell('AAA', T0 + D, '404', '0.25'), buy('BBB', T0 + 1, '55.21', '0.724506430')]),
               split_market, splits: [{ 'sym' => 'AAA', 'at' => SPLIT_AT, 'ratio' => '4:1' }]), # nothing left to restate
      scenario('split_old_row', NOW, basket(two, [buy('AAA', T0, '400', '0.25', asset: false), buy('BBB', T0 + 1, '55.21', '0.724506430')]),
               split_market, splits: [{ 'sym' => 'AAA', 'at' => SPLIT_AT, 'ratio' => '4:1' }]),

      # Live prices: #metrics_with_current_prices.
      scenario('prices_refused', NOW, basket(two, WEEKLY), TWO.merge(STOCK_PRICES => REFUSED)),
      scenario('prices_certificate', NOW, basket(two, WEEKLY), TWO.merge(STOCK_PRICES => CERTIFICATE)),
      scenario('prices_server_error', NOW, basket(two, WEEKLY), TWO.merge(STOCK_PRICES => SERVER_ERROR)),
      scenario('price_missing', NOW, basket(two, WEEKLY), TWO.merge(market(prices: { 'AAA' => 104.52 }).slice(STOCK_PRICES))),
      scenario('price_untraded', NOW, basket(two, WEEKLY), TWO.merge(market(prices: { 'AAA' => 104.52, 'BBB' => nil }).slice(STOCK_PRICES))),
      scenario('price_whole_number', NOW, basket(two, WEEKLY), TWO.merge(market(prices: { 'AAA' => 105, 'BBB' => 0.1 + 0.2 }).slice(STOCK_PRICES))),
      # Alpaca omits unreadable batch prices, keeping the other members available to value.
      scenario('price_nan', NOW, basket(two, WEEKLY), TWO.merge(market(prices: { 'AAA' => 104.52, 'BBB' => 'NaN' }).slice(STOCK_PRICES))),
      scenario('price_infinity', NOW, basket(%w[BTC ETH], crypto_orders), crypto_market('USD').merge(CRYPTO_PRICES => ok('trades' => { 'BTC/USD' => { 'p' => 'Infinity' }, 'ETH/USD' => { 'p' => 3020 } }))),
      # Strict venue parsing rejects a numeric prefix followed by garbage too.
      scenario('price_unreadable', NOW, basket(two, WEEKLY), TWO.merge(market(prices: { 'AAA' => 104.52, 'BBB' => '12abc' }).slice(STOCK_PRICES))),
      scenario('candle_nan', NOW, basket(two, WEEKLY),
               market(prices: { 'AAA' => 104.52, 'BBB' => 53.9 }, stock_bars: { 'AAA' => AAA_BARS, 'BBB' => BBB_BARS.first(5) + [[BBB_BARS[5][0], 'NaN']] + BBB_BARS.drop(6) })),
      scenario('lone_unpriced', NOW, basket(%w[AAA], WEEKLY.select { |o| o['sym'] == 'AAA' }),
               market(prices: { 'ZZZ' => 1 }, stock_bars: { 'AAA' => AAA_BARS })),
      scenario('delisted_member', NOW, basket(two, WEEKLY), TWO, delist: %w[BBB]),
      # A rotation: AAA is bought, sold whole, and delisted since. Nothing of it is held, so the live pass has nothing
      # to leave out; the chart leaves it out of the points at which it was held.
      scenario('sold_out_then_delisted', NOW,
               basket(two, [buy('AAA', T0, '101.37', '0.5'), buy('BBB', T0 + 1, '55.21', '0.7'),
                            sell('AAA', T0 + (3 * D), '103', '0.5', type: 'LIQUIDATION'), buy('BBB', T0 + (7 * D), '54.02', '0.7')]),
               TWO, delist: %w[AAA]),
      # An index bot asks for every ticker of its venue. A price that is no number, for one it never held, is not
      # used by Rails and is not this library's concern either.
      scenario('price_nan_unused', NOW, index(two, WEEKLY), index_market(two, extra_prices: { 'CCC' => 'NaN', 'DDD' => 'Infinity' })),
      scenario('all_delisted', NOW, basket(two, WEEKLY), market(prices: nil, crypto: nil), delist: %w[AAA BBB]),
      # The first points hold only a holding the bot cannot price: the chart reads the row before, then the fill marks.
      scenario('unpriceable_first', NOW, basket(%w[LLL AAA], [buy('LLL', T0, '200', '0.05'), buy('LLL', T0 + 60, '201', '0.05')] + WEEKLY.select { |o| o['sym'] == 'AAA' }),
               market(prices: { 'AAA' => 104.52 }, stock_bars: { 'AAA' => AAA_BARS }), delist: %w[LLL]),

      # The chart: Bot::ChartSeries and #metrics_with_current_prices_and_candles.
      aged('candles_1min', 7200.5, 60, 130), aged('candles_5min', 36_000, 300, 125), aged('candles_15min', 180_000, 900, 205),
      aged('candles_30min', 360_000, 1800, 205), aged('candles_1hour', 720_000, 3600, 205), aged('candles_1day', 20 * D, D, 25),
      aged('candles_at_the_boundary', 18_000, 300, 65), # exactly 300 minutes old is no longer "under"
      scenario('candles_none', NOW, basket(two, WEEKLY), market(prices: { 'AAA' => 104.52, 'BBB' => 53.9 }, stock_bars: { 'AAA' => [], 'BBB' => [] })),
      scenario('candles_one_failed', NOW, basket(two, WEEKLY), TWO.merge("#{ALPACA}/v2/stocks/BBB/bars" => SERVER_ERROR)),
      scenario('candles_one_refused', NOW, basket(two, WEEKLY), TWO.merge("#{ALPACA}/v2/stocks/BBB/bars" => REFUSED, "#{ALPACA}/v2/stocks/BBB/bars?adjustment=split" => REFUSED)),
      scenario('candles_begin_late', NOW, basket(two, WEEKLY),
               market(prices: { 'AAA' => 104.52, 'BBB' => 53.9 }, stock_bars: { 'AAA' => AAA_BARS, 'BBB' => BBB_BARS.last(6) })),
      scenario('candles_end_early', NOW, basket(two, WEEKLY),
               market(prices: { 'AAA' => 104.52 }, stock_bars: { 'AAA' => AAA_BARS, 'BBB' => BBB_BARS.first(6) })),
      scenario('candles_unsorted', NOW, basket(two, WEEKLY),
               market(prices: { 'AAA' => 104.52, 'BBB' => 53.9 }, stock_bars: { 'AAA' => AAA_BARS.reverse + AAA_BARS.first(2), 'BBB' => BBB_BARS })),
      scenario('candles_still_open', T0 + (9 * D) + 50_000, basket(two, WEEKLY), TWO), # the last bars are not closed yet
      scenario('many_buys', T0 + (40 * D), basket(%w[AAA], Array.new(620) { |i| buy('AAA', T0 + (i * 5400), (100 + (i * 0.01)).round(2).to_s, '0.01') }),
               market(prices: { 'AAA' => 107.5 }, stock_bars: { 'AAA' => daily(BARS_START, 44, 100.0, 0.15) })),
      scenario('whole_units', NOW, basket(%w[BTC], [buy('BTC', T0, '64000', '0.001'), buy('BTC', T0 + D, '65000.5', '0.0013')], quote: 'USDT'),
               crypto_market('USDT').merge(hosted(prices: { %w[tether usd] => 1 })), provider: 'deltabadger'),
      scenario('hidden_balances', NOW, basket(two, WEEKLY), TWO, hide_balances: true, time_zone: 'Kathmandu'),

      # The account's totals: User#global_pnl, User::PnlHistory, Bot#profit_in_usd, Denomination.
      account('deltabadger', hosted(rates: RATES, prices: { %w[tether usd] => 0.9993 })),
      account('coingecko', coingecko(rates: RATES, prices: { %w[tether usd] => 0.9993 })),
      # A coin's price that came as a String: a bot's profit and the history convert it, the account's total raises.
      account('deltabadger', hosted(rates: RATES, prices: { %w[tether usd] => '0.9993' }), name: 'account_string_price_hosted'),
      account('coingecko', coingecko(rates: RATES, prices: { %w[tether usd] => '0.9993' }), name: 'account_string_price_coingecko'),
      account('deltabadger', hosted(rates: RATES.except('eur')).merge("#{HOSTED}/prices?coin_ids=tether&vs_currencies=usd" => ok('data' => { 'tether' => {} })),
              name: 'account_rate_missing'),
      account('deltabadger', { "#{HOSTED}/exchange_rates" => SERVER_ERROR, "#{HOSTED}/prices?coin_ids=tether&vs_currencies=usd" => SERVER_ERROR }, name: 'account_rates_down'),
      account('deltabadger', hosted(rates: RATES).merge("#{HOSTED}/prices?coin_ids=tether&vs_currencies=usd" =>
        REFUSED.merge('message' => REFUSED['message'].sub('data.alpaca.markets', 'data-api'))), name: 'account_rates_refused'),
      scenario('account_whole_rates', NOW, [basket(two, WEEKLY)], TWO.merge(hosted(rates: { 'usd' => 64_000, 'pln' => 256_000 })), provider: 'deltabadger', display_currency: 'PLN'),
      # The same for a rate: the provider's table has one that is no number, for a currency nobody asked about.
      scenario('account_rate_nan_unused', NOW, [basket(two, WEEKLY)], TWO.merge(hosted(rates: RATES.merge('xau' => 'NaN'))), provider: 'deltabadger', display_currency: 'EUR'),
      scenario('account_no_provider', NOW, [basket(two, WEEKLY)], TWO, display_currency: 'EUR'),
      scenario('account_pair_bot', NOW, [basket(two, WEEKLY), { 'type' => 'single', 'members' => %w[AAA], 'label' => 'pair', 'orders' => [buy('AAA', T0, '100', '1')] }],
               TWO.merge("#{ALPACA}/v2/stocks/AAA/trades/latest" => ok('trade' => { 'p' => 104.52 }))),
      scenario('account_nothing_invested', NOW, [basket(two, [], label: 'a'), basket(%w[CCC], [], label: 'b')], market)
    ]
    list + (1..24).map { |seed| random_history(seed) }
  end

  # ---- commands -------------------------------------------------------------------------------------------------

  def grid(root)
    list = scenarios
    raise 'scenario names repeat' unless list.map { |sc| sc['name'] }.uniq.size == list.size

    template = Dir.mktmpdir('figures-world')
    asset_ids = world(template)
    list.each do |sc|
      dir = File.join(root, sc['name'])
      FileUtils.mkdir_p(dir)
      user, bots = build(sc.merge('dir' => dir), template, asset_ids)
      File.write(File.join(dir, 'scenario.json'), JSON.pretty_generate({
        'parity_scratch' => true, 'at' => time(sc['at']).utc.iso8601(9), 'user_id' => user.id, 'bot_ids' => bots.map(&:id),
        'provider' => sc['provider'], 'script' => sc['script']
      }.compact))
      ActiveRecord::Base.connection_pool.disconnect!
    end
    puts "built #{list.size} scenarios in #{root}"
  ensure
    FileUtils.rm_rf(template) if template
  end

  # What Rails answers, or what it raises: a transient market-data failure is raised to the caller's retry.
  def answer
    yield.to_json
  rescue StandardError => e
    { 'raised' => "#{e.class}: #{e.message}" }.to_json
  end

  # The chart's data, read off the partial every chart broadcast renders (Bot::Composition::Measurable#broadcast_chart).
  # The logos' images and colours are the page's; the library answers which asset stands behind each charted key.
  def chart(bot, metrics, user)
    html = ApplicationController.render(partial: 'bots/chart', locals: { bot:, metrics:, loading: false, current_user: user })
    node = Nokogiri::HTML5.fragment(html).at_css('[data-controller="bot--chart"]')
    return nil if node.nil?

    values = node.attributes.filter_map do |name, attribute|
      [name.delete_prefix('data-bot--chart-').delete_suffix('-value'), attribute.value] if name.start_with?('data-bot--chart-') && name.end_with?('-value')
    end.to_h.except('buy-logos')
    keys = (bot.chart_buy_marks.map { |mark| mark[1] } + (metrics[:chart][:prices] || {}).keys).uniq
    values.merge('logo-assets' => bot.chart_logo_assets(keys).compact.transform_values(&:id).to_json)
  end

  def figures(user, bot_ids)
    bots = bot_ids.to_h do |id|
      bot = Bot.find(id)
      next [id.to_s, { 'type' => bot.type }] unless bot.is_a?(Bot::Composition::Measurable)

      out = { 'metrics' => answer { bot.metrics }, 'live' => answer { bot.metrics_with_current_prices } }
      marked = nil
      out['marked'] = answer { marked = bot.metrics_with_current_prices_and_candles }
      out['chart'] = marked && chart(bot, marked, user)
      out['profit_in_usd'] = answer { bot.profit_in_usd(bot.metrics_with_current_prices, cache_only: false) }
      [id.to_s, out]
    end
    { 'bots' => bots,
      'global_pnl' => answer { user.global_pnl(use_cache: false) },
      'global_pnl_snapshot' => answer { user.global_pnl_snapshot(cache_only: false) },
      'pnl_history' => answer { User::PnlHistory.snapshot(user, live: true) },
      'denomination' => answer { d = Denomination.for(user.display_currency); { currency: d.currency, rate: d.rate } } }
  end

  def record(root)
    Rails.cache = ActiveSupport::Cache::MemoryStore.new # as production has one; development's null store would refetch everything
    ActiveSupport::JSON::Encoding.time_precision = 9 # every digit a Time carries, so two labels a microsecond apart differ
    Faraday::Adapter.lookup_middleware(:net_http_persistent).prepend(ScriptedMarket::Adapter)
    # The CoinGecko client asks the paid API whether a key is a paid one before its first call. The scenarios' key is a free one.
    Clients::Coingecko.instance_variable_set(:@detect_plan, { 'CG-demo-key' => :demo })
    Dir[File.join(root, '*/scenario.json')].sort.each do |path|
      dir = File.dirname(path)
      sc = JSON.parse(File.read(path))
      ActiveRecord::Base.establish_connection(adapter: 'sqlite3', database: File.join(dir, 'production.sqlite3'))
      Rails.cache.clear
      MarketData.instance_variable_set(:@client, nil)
      MarketData.instance_variable_set(:@coingecko, nil)
      ScriptedMarket.http = sc['script']
      ScriptedMarket.requests = []
      ScriptedMarket.gaps = []
      started = Process.clock_gettime(Process::CLOCK_MONOTONIC)
      out = begin
        travel_to(Time.iso8601(sc['at']), with_usec: true) { figures(User.find(sc['user_id']), sc['bot_ids']) }
      rescue Harness::Unscripted => e
        raise Harness::Unscripted, "#{File.basename(dir)}: #{e.message}"
      end
      raise Harness::Unscripted, "#{File.basename(dir)}: #{ScriptedMarket.gaps.uniq.join(', ')}" if ScriptedMarket.gaps.any?

      out['seconds'] = (Process.clock_gettime(Process::CLOCK_MONOTONIC) - started).round(3) # how long Rails took: printed beside this library's, never compared
      out['requests'] = ScriptedMarket.requests.uniq.sort
      File.write(File.join(dir, 'rails.json'), JSON.pretty_generate(out))
      travel_back
      ActiveRecord::Base.connection_pool.disconnect!
    end
    puts "recorded #{Dir[File.join(root, '*/rails.json')].size} scenarios in #{root}"
  end
end

unless defined?(FIGURES_LIBRARY)
  command, root = ARGV
  Rails.logger.level = :warn # a run reads and writes some 100,000 rows; development would log every one
  raise ArgumentError, 'usage: grid <root> | record <root>' unless root

  case command
  when 'grid' then Figures.grid(root)
  when 'record' then Figures.record(root)
  else raise ArgumentError, 'usage: grid <root> | record <root>'
  end
end
