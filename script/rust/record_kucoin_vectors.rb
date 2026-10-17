# Records Exchanges::Kucoin's sizing and order wire for rust/tests/amount.rs, and its #parse_order_data figures for
# rust/tests/kucoin.rs.
#   bin/rails runner script/rust/record_kucoin_vectors.rb rust/tests/fixtures/kucoin_vectors.json
# Nothing is saved and nothing leaves the machine: the honeymaker client is replaced by a recorder, and any real
# connection raises. Re-run (script/rust/repin) when a file under "sources" changes; the Rust tests fail until then.
require 'json'
require 'digest'

class Unscripted < Exception; end # rubocop:disable Lint/InheritException

Net::HTTP.prepend(Module.new { def connect = raise(Unscripted, "real connection to #{address}") })

out = {
  'sources' => %w[app/models/exchanges/kucoin.rb app/models/exchange.rb app/models/bot/order_setter.rb app/models/ticker.rb
                  script/rust/record_kucoin_vectors.rb]
    .to_h { |f| [f, Digest::SHA256.file(Rails.root.join(f)).hexdigest] },
  'gems' => { 'honeymaker' => Honeymaker::VERSION }
}

Rails.configuration.dry_run = false # the test environment trades dry; the real set_*_order is what is recorded
kucoin = Exchanges::Kucoin.new(name: 'KuCoin')
captured = nil
kucoin.instance_variable_set(:@client, Object.new.tap do |c|
  c.define_singleton_method(:place_order) { |**kw| captured = kw; Result::Success.new(order_id: '6716f0a2c1b6d10007a3b1c5', raw: {}) }
end)

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
rng = Random.new(20_261_017)
decimal = ->(min_exp, max_exp) { (BigDecimal(rng.rand(1...(10**rng.rand(1..9))).to_s) * (BigDecimal(10)**rng.rand(min_exp..max_exp))).to_s('F') }
pairs = Array.new(720) { [decimal.(-9, 6), decimal.(-5, 5)] }

sizing = []
tickers.each do |t|
  pairs.each do |price_s, x_s|
    %i[market_order limit_order].each do |order_type|
      ticker = Ticker.new(exchange: kucoin, ticker: 'BTC-USDT', base: 'BTC', quote: 'USDT', **t.transform_values { |v| v.is_a?(String) ? BigDecimal(v) : v })
      bot = Bots::DcaMultiAsset.new(exchange: kucoin)
      price = order_type == :limit_order ? ticker.adjusted_price(price: BigDecimal(price_s) * (1.to_d - 0.0025.to_d)) : BigDecimal(price_s)
      next if price.zero? # refused before sizing ("limit price rounds to zero"), on both sides

      x = BigDecimal(x_s)
      info = bot.send(:calculate_best_amount_info, { ticker:, price:, amount: x / price, quote_amount: x, side: :buy, order_type: })
      captured = nil
      placed = if order_type == :limit_order
                 kucoin.limit_buy(ticker:, amount: info[:amount], amount_type: info[:amount_type], price:)
               else
                 kucoin.market_buy(ticker:, amount: info[:amount], amount_type: info[:amount_type])
               end
      row = { 'ticker' => tickers.index(t), 'last_or_ask' => price_s, 'x' => x_s, 'order_type' => order_type.to_s,
              'price' => price.to_s('F'), 'amount' => (x / price).to_s('F'), 'amount_type' => info[:amount_type].to_s,
              'below_minimum' => info[:below_minimum_amount] }
      row['wire'] = captured&.except(:client_oid)&.compact&.transform_keys(&:to_s)&.transform_values(&:to_s)
      row['refused'] = placed.errors.join if captured.nil?
      sizing << row
    end
  end
end
out['kucoin_tickers'] = tickers.map { |t| t.transform_keys(&:to_s).transform_values(&:to_s) }
out['kucoin_sizing'] = sizing

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

# One case per line: the file stays reviewable in a diff and a third the size of pretty JSON.
lines = out.map { |k, v| v.is_a?(Array) ? "#{k.to_json}: [\n#{v.map { |r| "  #{JSON.generate(r)}" }.join(",\n")}\n]" : "#{k.to_json}: #{JSON.generate(v)}" }
File.write(ARGV.fetch(0), "{\n#{lines.join(",\n")}\n}\n")
puts "wrote #{ARGV.fetch(0)}: #{sizing.size} sizing cases, #{orders.size} orders"
