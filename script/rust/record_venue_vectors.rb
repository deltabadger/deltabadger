# Records a venue's sizing and order wire for rust/tests/amount.rs, and what else its Rust port replays (plan VB §1.8: one
# recorder, a section per venue):
#   bin/rails runner script/rust/record_venue_vectors.rb binance rust/tests/fixtures/binance_vectors.json  # + the fill rebuild (binance_trade.rs)
#   bin/rails runner script/rust/record_venue_vectors.rb kucoin rust/tests/fixtures/kucoin_vectors.json    # + #parse_order_data (kucoin.rs)
#   bin/rails runner script/rust/record_venue_vectors.rb prices rust/tests/fixtures/prices_vectors.json     # plan VB slice P (venue_rules.rs)
# Nothing is saved and nothing leaves the machine: the honeymaker client is replaced by a recorder, and any real
# connection raises. Re-run (script/rust/repin) when a file under "sources" changes; the Rust tests fail until then.
require 'json'
require 'digest'

class Unscripted < Exception; end # rubocop:disable Lint/InheritException

Net::HTTP.prepend(Module.new { def connect = raise(Unscripted, "real connection to #{address}") })

venue, out_path = ARGV.fetch(0), ARGV.fetch(1)
pin = ->(files) { files.to_h { |f| [f, Digest::SHA256.file(Rails.root.join(f)).hexdigest] } }
Rails.configuration.dry_run = false # the test environment trades dry; the real set_*_order is what is recorded
captured = nil
# Per venue: the header, the exchange with its client replaced by a recorder of the order it would send, the pair's name,
# the fields of the order that change per send, the RNG's seed, and the pairs' precisions as #get_tickers_info stores them.
case venue
when 'binance'
  out = { 'sources' => pin.(%w[app/models/exchanges/binance.rb app/models/exchange.rb app/models/bot/order_setter.rb app/models/ticker.rb app/models/utilities/math.rb]),
          'honeymaker' => Honeymaker::VERSION }
  exchange = Exchanges::Binance.new(name: 'Binance')
  exchange.instance_variable_set(:@client, Object.new.tap do |c|
    c.define_singleton_method(:new_order) { |**kw| captured = kw; Result::Success.new(order_id: "#{kw[:symbol]}-1", raw: {}) }
  end)
  pair, strip, seed = 'BTCUSDT', [], 20_261_015
  # Pairs shaped as Exchanges::Binance#get_tickers_info reads exchangeInfo: LOT_SIZE minQty / stepSize decimals, NOTIONAL
  # minNotional, quoteAssetPrecision, PRICE_FILTER tickSize decimals.
  tickers = [
    { minimum_base_size: '0.00001', minimum_quote_size: '5', base_decimals: 5, quote_decimals: 8, price_decimals: 2 },
    { minimum_base_size: '0.0001', minimum_quote_size: '0.0001', base_decimals: 4, quote_decimals: 8, price_decimals: 5 },
    { minimum_base_size: '1', minimum_quote_size: '5', base_decimals: 0, quote_decimals: 8, price_decimals: 8 },
    { minimum_base_size: '1', minimum_quote_size: '1', base_decimals: 0, quote_decimals: 8, price_decimals: 5 },
    { minimum_base_size: '0.001', minimum_quote_size: '5', base_decimals: 3, quote_decimals: 8, price_decimals: 1 },
    { minimum_base_size: '0.01', minimum_quote_size: '10', base_decimals: 2, quote_decimals: 2, price_decimals: 3 },
    { minimum_base_size: '1', minimum_quote_size: '500', base_decimals: 0, quote_decimals: 0, price_decimals: 0 },
    { minimum_base_size: '0.00005', minimum_quote_size: '0.5', base_decimals: 8, quote_decimals: 5, price_decimals: 1 }
  ]
when 'kucoin'
  out = { 'sources' => pin.(%w[app/models/exchanges/kucoin.rb app/models/exchange.rb app/models/bot/order_setter.rb app/models/ticker.rb
                               script/rust/record_venue_vectors.rb]),
          'gems' => { 'honeymaker' => Honeymaker::VERSION } }
  exchange = Exchanges::Kucoin.new(name: 'KuCoin')
  exchange.instance_variable_set(:@client, Object.new.tap do |c|
    c.define_singleton_method(:place_order) { |**kw| captured = kw; Result::Success.new(order_id: '6716f0a2c1b6d10007a3b1c5', raw: {}) }
  end)
  pair, strip, seed = 'BTC-USDT', %i[client_oid], 20_261_017
  # Pairs shaped as Exchanges::Kucoin#get_tickers_info reads /api/v2/symbols: baseMinSize, quoteMinSize and the decimals of
  # baseIncrement, quoteIncrement and priceIncrement.
  tickers = [
    { minimum_base_size: '0.00001', minimum_quote_size: '0.1', base_decimals: 8, quote_decimals: 6, price_decimals: 1 },
    { minimum_base_size: '0.0001', minimum_quote_size: '0.01', base_decimals: 7, quote_decimals: 7, price_decimals: 2 },
    { minimum_base_size: '1', minimum_quote_size: '0.1', base_decimals: 0, quote_decimals: 6, price_decimals: 5 },
    { minimum_base_size: '0.01', minimum_quote_size: '10', base_decimals: 2, quote_decimals: 2, price_decimals: 4 },
    { minimum_base_size: '0.001', minimum_quote_size: '1', base_decimals: 4, quote_decimals: 8, price_decimals: 1 },
    { minimum_base_size: '10', minimum_quote_size: '0.1', base_decimals: 0, quote_decimals: 4, price_decimals: 6 },
    { minimum_base_size: '0.1', minimum_quote_size: '5', base_decimals: 1, quote_decimals: 0, price_decimals: 0 },
    { minimum_base_size: '0.00005', minimum_quote_size: '0.5', base_decimals: 8, quote_decimals: 5, price_decimals: 3 }
  ]
when 'prices'
  # Not one venue: every venue's Exchange#adjusted_price and #market_price_for, the default rule and its two overrides.
  price_venues = %w[Kraken Alpaca Binance Kucoin Gemini Hyperliquid BinanceUs]
  out = { 'sources' => pin.(%w[app/models/exchange.rb app/models/ticker.rb script/rust/record_venue_vectors.rb] +
                            price_venues.map { |v| "app/models/exchanges/#{v.underscore}.rb" }) }
  seed, tickers = 20_261_020, []
else raise ArgumentError, "unknown venue #{venue}"
end
rng = Random.new(seed)
decimal = ->(min_exp, max_exp) { (BigDecimal(rng.rand(1...(10**rng.rand(1..9))).to_s) * (BigDecimal(10)**rng.rand(min_exp..max_exp))).to_s('F') }
pairs = venue == 'prices' ? [] : Array.new(720) { [decimal.(-9, 6), decimal.(-5, 5)] }

# Bot::OrderSetter's sizing at a market (the ask) or limit (the last, adjusted) price, then the order the venue would send.
sizing = []
tickers.each do |t|
  pairs.each do |price_s, x_s|
    %i[market_order limit_order].each do |order_type|
      ticker = Ticker.new(exchange:, ticker: pair, base: 'BTC', quote: 'USDT', **t.transform_values { |v| v.is_a?(String) ? BigDecimal(v) : v })
      bot = Bots::DcaMultiAsset.new(exchange:)
      price = order_type == :limit_order ? ticker.adjusted_price(price: BigDecimal(price_s) * (1.to_d - 0.0025.to_d)) : BigDecimal(price_s)
      next if price.zero? # refused before sizing ("limit price rounds to zero"), on both sides

      x = BigDecimal(x_s)
      info = bot.send(:calculate_best_amount_info, { ticker:, price:, amount: x / price, quote_amount: x, side: :buy, order_type: })
      captured = nil
      placed = if order_type == :limit_order
                 exchange.limit_buy(ticker:, amount: info[:amount], amount_type: info[:amount_type], price:)
               else
                 exchange.market_buy(ticker:, amount: info[:amount], amount_type: info[:amount_type])
               end
      row = { 'ticker' => tickers.index(t), 'last_or_ask' => price_s, 'x' => x_s, 'order_type' => order_type.to_s,
              'price' => price.to_s('F'), 'amount' => (x / price).to_s('F'), 'amount_type' => info[:amount_type].to_s,
              'below_minimum' => info[:below_minimum_amount] }
      row['wire'] = captured&.except(*strip)&.compact&.transform_keys(&:to_s)&.transform_values(&:to_s)
      row['refused'] = placed.errors.join if captured.nil?
      sizing << row
    end
  end
end
unless venue == 'prices'
  out["#{venue}_tickers"] = tickers.map { |t| t.transform_keys(&:to_s).transform_values(&:to_s) }
  out["#{venue}_sizing"] = sizing
end

# What else the venue's port replays.
case venue
when 'binance'
  binance = exchange
  # Honeymaker::Clients::Binance's signed requests under a frozen clock (#timestamp_ms): the URL query each one sends, signature
  # included, captured at its Faraday adapter. Nothing is sent.
  module SigningRecorder
    mattr_accessor :seen

    module Adapter
      def call(env)
        raise Unscripted, "not Binance: #{env.url}" unless env.url.host == 'api.binance.com'

        SigningRecorder.seen = { 'method' => env.method.to_s.upcase, 'path' => env.url.path, 'query' => env.url.query,
                                 'key_header' => env.request_headers['X-MBX-APIKEY'], 'body' => env.request_body }
        env.response = Faraday::Response.new
        save_response(env, 200, '{}', { 'Content-Type' => 'application/json' })
        @app.call(env)
      end
    end
  end
  Faraday::Adapter.lookup_middleware(:net_http_persistent).prepend(SigningRecorder::Adapter)
  uuid = '5f1c1f3e-2b8f-4c4e-9a59-0d9c3f1b6a01'
  signing_calls = [
    ['POST', '/api/v3/order', :new_order, { symbol: 'BTCUSDT', side: 'BUY', type: 'MARKET', quote_order_qty: '60.5', new_client_order_id: uuid,
                                            self_trade_prevention_mode: 'EXPIRE_MAKER' }],
    ['POST', '/api/v3/order', :new_order, { symbol: 'BTCUSDT', side: 'BUY', type: 'MARKET', quantity: '0.00123', new_client_order_id: uuid,
                                            self_trade_prevention_mode: 'EXPIRE_MAKER' }],
    ['POST', '/api/v3/order', :new_order, { symbol: 'ETHBTC', side: 'BUY', type: 'LIMIT', time_in_force: 'GTC', price: '0.03512',
                                            quantity: '1.2', new_client_order_id: uuid }],
    ['GET', '/api/v3/order', :query_order, { symbol: 'BTCUSDT', order_id: '28457', recv_window: 60_000 }],
    ['GET', '/api/v3/order', :query_order, { symbol: 'BTCUSDT', orig_client_order_id: uuid, recv_window: 60_000 }],
    ['GET', '/api/v3/allOrders', :all_orders, { symbol: 'BTCUSDT', order_id: 28_457, limit: 1000, recv_window: 60_000 }],
    ['GET', '/api/v3/myTrades', :account_trade_list, { symbol: 'BTCUSDT', order_id: 28_457, from_id: 0, limit: 1000, recv_window: 60_000 }],
    ['GET', '/api/v3/account', :account_information, { omit_zero_balances: true, recv_window: 60_000 }]
  ]
  out['signing'] = [['binance-key', 'NhqPtmdSJYdKjVHjA7PZj4Mge3R5YNiP1e3UZjInClVN65XAbvqqM6A7H5fATj0j'], ['k', 's3cr3t/+=']].flat_map do |key, secret|
    [1_789_871_400_750, 1_499_827_319_559].flat_map do |stamp|
      signing_calls.map do |method, path, call, args|
        client = Honeymaker::Clients::Binance.new(api_key: key, api_secret: secret)
        client.define_singleton_method(:timestamp_ms) { stamp }
        SigningRecorder.seen = nil
        client.public_send(call, **args)
        seen = SigningRecorder.seen or raise "#{call}: nothing sent"
        raise "#{call}: #{seen['method']} #{seen['path']}" unless [seen['method'], seen['path']] == [method, path]

        { 'api_key' => key, 'api_secret' => secret, 'timestamp' => stamp, 'call' => call.to_s, 'args' => args.transform_keys(&:to_s).transform_values(&:to_s) }
          .merge(seen)
      end
    end
  end

  # #parse_trade_data and #aggregate_trades over generated myTrades pages: commission in the base, the quote, BNB or nothing, on
  # a listed pair or a renamed one (the symbol fallback), the weighted price over a Float divisor, rounded to the tick.
  listed = {
    'BTCUSDT' => Ticker.new(exchange: binance, ticker: 'BTCUSDT', base: 'BTC', quote: 'USDT', price_decimals: 2),
    'ETHBTC' => Ticker.new(exchange: binance, ticker: 'ETHBTC', base: 'ETH', quote: 'BTC', price_decimals: 5)
  }
  binance.define_singleton_method(:tickers) { Object.new.tap { |t| t.define_singleton_method(:find_by) { |ticker:| listed[ticker] } } }
  symbols = { 'BTCUSDT' => %w[BTC USDT], 'ETHBTC' => %w[ETH BTC], 'RNDRUSDT' => %w[RNDR USDT] }
  fills = Array.new(10_000) do |n|
    symbol = symbols.keys[n % 3]
    base, quote = symbols[symbol]
    buyer = rng.rand(10) < 8
    trades = Array.new(rng.rand(1..4)) do |i|
      price = decimal.(-6, 5)
      qty = decimal.(-8, 3)
      asset = [base, quote, 'BNB', base, quote].sample(random: rng)
      { 'symbol' => symbol, 'id' => 1000 + i, 'orderId' => 28_457, 'price' => price, 'qty' => qty,
        'quoteQty' => (BigDecimal(price) * BigDecimal(qty)).round(8).to_s('F'), 'commission' => decimal.(-9, -1), 'commissionAsset' => asset,
        'isBuyer' => buyer, 'isMaker' => rng.rand(2).zero? }
    end
    trades = [1.00, 1.01, 1.00].each_with_index.map { |p, i| trades[0].merge('id' => 2000 + i, 'price' => format('%.2f', p), 'qty' => '1') } if n == 0
    parsed = trades.map { |t| binance.send(:parse_trade_data, t) }
    aggregated = binance.send(:aggregate_trades, parsed)
    # Compact: the symbol once, each trade as [id, price, qty, quoteQty, commission, commissionAsset, isBuyer, isMaker] (orderId
    # 28457); per trade its net [amount_exec, quote_amount_exec]; the aggregate [price, amount_exec, quote_amount_exec].
    { 'symbol' => symbol,
      'trades' => trades.map { |t| t.values_at('id', 'price', 'qty', 'quoteQty', 'commission', 'commissionAsset', 'isBuyer', 'isMaker') },
      'net' => parsed.map { |t| [t[:amount_exec].to_d.to_s('F'), t[:quote_amount_exec].to_d.to_s('F')] },
      'aggregate' => %i[price amount_exec quote_amount_exec].map { |k| aggregated[k].to_d.to_s('F') } }
  end
  out['binance_fills'] = fills
  extra = "#{fills.size} fills"
when 'kucoin'
  kucoin = exchange
  # #parse_order_data over generated GET /api/v1/orders/{id} answers: the price dealFunds / dealSize rounded half up to the
  # listed pair's tick (or left whole for an unlisted one), else the order's own price; each status flag combination.
  listed = { 'BTC-USDT' => Ticker.new(exchange: kucoin, ticker: 'BTC-USDT', base: 'BTC', quote: 'USDT', price_decimals: 1),
             'ETH-BTC' => Ticker.new(exchange: kucoin, ticker: 'ETH-BTC', base: 'ETH', quote: 'BTC', price_decimals: 6) }
  kucoin.define_singleton_method(:tickers) { Object.new.tap { |t| t.define_singleton_method(:find_by) { |ticker:| listed[ticker] } } }
  symbols = %w[BTC-USDT ETH-BTC NEW-USDT]
  flags = [true, false, nil]
  orders = Array.new(10_000) do |n|
    dealt = rng.rand(3).positive?
    raw = { 'id' => "o#{n}", 'symbol' => symbols[n % 3], 'type' => %w[market limit LIMIT Market][n % 4], 'side' => %w[buy sell BUY][n % 3],
            'price' => rng.rand(4).zero? ? '0' : decimal.(-6, 5), 'size' => decimal.(-8, 3), 'funds' => decimal.(-4, 4),
            'dealSize' => dealt ? decimal.(-8, 3) : '0', 'dealFunds' => dealt ? decimal.(-6, 5) : '0',
            'isActive' => flags[rng.rand(3)], 'cancelExist' => flags[rng.rand(3)], 'cancelReason' => '' }.compact
    data = kucoin.send(:parse_order_data, raw['id'], raw)
    { 'raw' => raw, 'status' => data[:status].to_s, 'price' => data[:price]&.to_d&.to_s('F'),
      'amount' => data[:amount]&.to_d&.to_s('F'), 'quote_amount' => data[:quote_amount]&.to_d&.to_s('F'),
      'amount_exec' => data[:amount_exec].to_d.to_s('F'), 'quote_amount_exec' => data[:quote_amount_exec].to_d.to_s('F'),
      'side' => data[:side].to_s, 'order_type' => data[:order_type].to_s }
  end
  out['kucoin_orders'] = orders
  extra = "#{orders.size} orders"
when 'prices'
  # Ticker#adjusted_price (Exchange#adjusted_price: price_decimals; Hyperliquid's 5 significant figures within
  # 8 - base_decimals places) for floor, ceil and round, 1e-9..1e7, and Exchange#market_price_for over the same touch
  # prices: adjusted_price(touch), and on Gemini and Hyperliquid adjusted_price(that × (1 ± MARKET_CROSS)) on top.
  # A row is [venue, ticker, price, method or side, result]; a raise is recorded as "error: <class>".
  out['price_tickers'] = [[0, 0], [2, 2], [5, 5], [8, 8], [1, 9], [4, 3], [6, 0]].map { |p, b| { 'price_decimals' => p, 'base_decimals' => b } }
  special = %w[1.239 1.231 1.2345 0.00000001 0.000012345678 0.0234 3.5 66.6 9999.95 10000 12345 12345.6 99999.5 99999 100000 123456
               123456.789 1000000 9999999.99 0.99999 1.000005]
  prices = special + Array.new(130) { decimal.(-9, 6) }
  result = lambda do |&value|
    value.call.to_d.to_s('F')
  rescue StandardError => e
    "error: #{e.class}"
  end
  adjusted, market = [], []
  price_venues.each do |name|
    exchange = "Exchanges::#{name}".constantize.new(name:)
    out['price_tickers'].each_with_index do |t, i|
      prices.each do |p|
        ticker = Ticker.new(exchange:, ticker: 'BTCUSD', base: 'BTC', quote: 'USD', **t.transform_keys(&:to_sym))
        %i[floor ceil round].each { |m| adjusted << [name, i, p, m.to_s, result.call { ticker.adjusted_price(price: BigDecimal(p), method: m) }] }
        ticker.define_singleton_method(:get_ask_price) { Result::Success.new(BigDecimal(p)) }
        ticker.define_singleton_method(:get_bid_price) { Result::Success.new(BigDecimal(p)) }
        %i[buy sell].each { |side| market << [name, i, p, side.to_s, result.call { exchange.market_price_for(ticker:, side:).data }] }
      end
    end
  end
  out['adjusted_price'] = adjusted
  out['market_price_for'] = market
  extra = "#{adjusted.size} adjusted prices, #{market.size} market prices"
end

# One case per line: the file stays reviewable in a diff and a third the size of pretty JSON.
lines = out.map { |k, v| v.is_a?(Array) ? "#{k.to_json}: [\n#{v.map { |r| "  #{JSON.generate(r)}" }.join(",\n")}\n]" : "#{k.to_json}: #{JSON.generate(v)}" }
File.write(out_path, "{\n#{lines.join(",\n")}\n}\n")
puts "wrote #{out_path}: #{sizing.size} sizing cases, #{extra}"
