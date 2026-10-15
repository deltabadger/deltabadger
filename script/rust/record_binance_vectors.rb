# Records Exchanges::Binance's sizing and order wire for rust/tests/amount.rs (and its fill rebuild for rust/tests/binance_trade.rs).
#   bin/rails runner script/rust/record_binance_vectors.rb rust/tests/fixtures/binance_vectors.json
# Nothing is saved and nothing leaves the machine: the honeymaker client is replaced by a recorder, and any real
# connection raises. Re-run (script/rust/repin) when a file under "sources" changes; the Rust tests fail until then.
require 'json'
require 'digest'

class Unscripted < Exception; end # rubocop:disable Lint/InheritException

Net::HTTP.prepend(Module.new { def connect = raise(Unscripted, "real connection to #{address}") })

out = {
  'sources' => %w[app/models/exchanges/binance.rb app/models/exchange.rb app/models/bot/order_setter.rb app/models/ticker.rb]
    .to_h { |f| [f, Digest::SHA256.file(Rails.root.join(f)).hexdigest] },
  'honeymaker' => Honeymaker::VERSION
}

Rails.configuration.dry_run = false # the test environment trades dry; the real set_*_order is what is recorded
binance = Exchanges::Binance.new(name: 'Binance')
captured = nil
binance.instance_variable_set(:@client, Object.new.tap do |c|
  c.define_singleton_method(:new_order) { |**kw| captured = kw; Result::Success.new(order_id: "#{kw[:symbol]}-1", raw: {}) }
end)

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
rng = Random.new(20_261_015)
decimal = ->(min_exp, max_exp) { (BigDecimal(rng.rand(1...(10**rng.rand(1..9))).to_s) * (BigDecimal(10)**rng.rand(min_exp..max_exp))).to_s('F') }
pairs = Array.new(720) { [decimal.(-9, 6), decimal.(-5, 5)] }

sizing = []
tickers.each do |t|
  pairs.each do |price_s, x_s|
    %i[market_order limit_order].each do |order_type|
      ticker = Ticker.new(exchange: binance, ticker: 'BTCUSDT', base: 'BTC', quote: 'USDT', **t.transform_values { |v| v.is_a?(String) ? BigDecimal(v) : v })
      bot = Bots::DcaMultiAsset.new(exchange: binance)
      price = order_type == :limit_order ? ticker.adjusted_price(price: BigDecimal(price_s) * (1.to_d - 0.0025.to_d)) : BigDecimal(price_s)
      next if price.zero? # refused before sizing ("limit price rounds to zero"), on both sides

      x = BigDecimal(x_s)
      info = bot.send(:calculate_best_amount_info, { ticker:, price:, amount: x / price, quote_amount: x, side: :buy, order_type: })
      captured = nil
      placed = if order_type == :limit_order
                 binance.limit_buy(ticker:, amount: info[:amount], amount_type: info[:amount_type], price:)
               else
                 binance.market_buy(ticker:, amount: info[:amount], amount_type: info[:amount_type])
               end
      row = { 'ticker' => t.transform_values(&:to_s), 'last_or_ask' => price_s, 'x' => x_s, 'order_type' => order_type.to_s,
              'price' => price.to_s('F'), 'amount' => (x / price).to_s('F'), 'amount_type' => info[:amount_type].to_s,
              'below_minimum' => info[:below_minimum_amount] }
      row['wire'] = captured&.compact&.transform_keys(&:to_s)&.transform_values(&:to_s)
      row['refused'] = placed.errors.join if captured.nil?
      sizing << row
    end
  end
end
out['binance_sizing'] = sizing

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

# One case per line: the file stays reviewable in a diff and a third the size of pretty JSON.
lines = out.map { |k, v| v.is_a?(Array) ? "#{k.to_json}: [\n#{v.map { |r| "  #{JSON.generate(r)}" }.join(",\n")}\n]" : "#{k.to_json}: #{JSON.generate(v)}" }
File.write(ARGV.fetch(0), "{\n#{lines.join(",\n")}\n}\n")
puts "wrote #{ARGV.fetch(0)}: #{sizing.size} sizing cases"
