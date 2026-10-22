# BingX sizing and parsing oracle; independent entry point preserves existing venue pins.
require 'json'
require 'digest'

class Unscripted < Exception; end # rubocop:disable Lint/InheritException

Net::HTTP.prepend(Module.new { def connect = raise(Unscripted, "real connection to #{address}") })

venue, out_path = 'bingx', ARGV.fetch(0)
pin = ->(files) { files.to_h { |f| [f, Digest::SHA256.file(Rails.root.join(f)).hexdigest] } }
Rails.configuration.dry_run = false # the test environment trades dry; the real set_*_order is what is recorded
captured = nil
# Per venue: the header, the exchange with its client replaced by a recorder of the order it would send, the pair's name,
# the fields of the order that change per send, the RNG's seed, and the pairs' precisions as #get_tickers_info stores them.
  out = { 'sources' => pin.(%w[app/models/exchanges/bingx.rb app/models/exchange.rb app/models/bot/order_setter.rb app/models/ticker.rb
                               script/rust/record_bingx_vectors.rb]),
          'gems' => { 'honeymaker' => Honeymaker::VERSION } }
  exchange = Exchanges::Bingx.new(name: 'BingX')
  exchange.instance_variable_set(:@client, Object.new.tap do |c|
    c.define_singleton_method(:place_order) { |**kw| captured = kw; Result::Success.new(order_id: "#{kw[:symbol]}-1", raw: {}) }
  end)
  pair, strip, seed = 'BTC-USDT', [], 20_261_021
  # Synthetic LOT_SIZE/minVal and decimal boundaries, including zero minimums.
  tickers = [
    { minimum_base_size: '0.000001', minimum_quote_size: '1', base_decimals: 6, quote_decimals: 2, price_decimals: 2 },
    { minimum_base_size: '0', minimum_quote_size: '1', base_decimals: 2, quote_decimals: 8, price_decimals: 10 },
    { minimum_base_size: '0', minimum_quote_size: '5', base_decimals: 0, quote_decimals: 6, price_decimals: 8 },
    { minimum_base_size: '0.0001', minimum_quote_size: '0.0001', base_decimals: 4, quote_decimals: 8, price_decimals: 6 },
    { minimum_base_size: '1', minimum_quote_size: '5', base_decimals: 0, quote_decimals: 4, price_decimals: 5 },
    { minimum_base_size: '0.01', minimum_quote_size: '10', base_decimals: 2, quote_decimals: 2, price_decimals: 3 },
    { minimum_base_size: '0', minimum_quote_size: '0', base_decimals: 8, quote_decimals: 2, price_decimals: 1 },
    { minimum_base_size: '0.00005', minimum_quote_size: '0.5', base_decimals: 8, quote_decimals: 5, price_decimals: 1 }
  ]
rng = Random.new(seed)
decimal = ->(min_exp, max_exp) { (BigDecimal(rng.rand(1...(10**rng.rand(1..9))).to_s) * (BigDecimal(10)**rng.rand(min_exp..max_exp))).to_s('F') }
pairs = Array.new(720) { [decimal.(-9, 6), decimal.(-5, 5)] }

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

  bingx = exchange
  # #parse_order_data over generated trade/query answers (B1: the fills' average whenever
  # something filled, rounded half up to the listed pair's tick or left whole for an unlisted one), every status
  # ORDER_STATUS_MAP lists, both types and sides, origQuoteOrderQty present or absent and a negative cumulative quote.
  listed = { 'BTC-USDT' => Ticker.new(exchange: bingx, ticker: 'BTC-USDT', base: 'BTC', quote: 'USDT', price_decimals: 2),
             'PEPE-USDT' => Ticker.new(exchange: bingx, ticker: 'PEPE-USDT', base: 'PEPE', quote: 'USDT', price_decimals: 10) }
  bingx.define_singleton_method(:tickers) { Object.new.tap { |t| t.define_singleton_method(:find_by) { |ticker:| listed[ticker] } } }
  statuses = Exchanges::Bingx::ORDER_STATUS_MAP.keys
  orders = Array.new(10_000) do |n|
    filled = rng.rand(3).positive?
    raw = { 'symbol' => %w[BTC-USDT PEPE-USDT NEW-USDT][n % 3], 'orderId' => "#{n}", 'type' => %w[MARKET LIMIT][n % 2], 'side' => %w[BUY SELL][rng.rand(2)],
            'status' => statuses[rng.rand(statuses.size)], 'price' => rng.rand(4).zero? ? '0' : decimal.(-9, 5), 'origQty' => rng.rand(5).zero? ? '0' : decimal.(-8, 3),
            'origQuoteOrderQty' => [nil, '0', decimal.(-4, 4)][rng.rand(3)],
            'executedQty' => filled ? decimal.(-8, 3) : '0', 'cummulativeQuoteQty' => filled ? decimal.(-6, 5) : (rng.rand(9).zero? ? '-1' : '0') }.compact
    data = bingx.send(:parse_order_data, "#{raw['symbol']}-#{raw['orderId']}", raw)
    { 'raw' => raw, 'status' => data[:status].to_s, 'price' => data[:price]&.to_d&.to_s('F'),
      'amount' => data[:amount]&.to_d&.to_s('F'), 'quote_amount' => data[:quote_amount]&.to_d&.to_s('F'),
      'amount_exec' => data[:amount_exec].to_d.to_s('F'), 'quote_amount_exec' => data[:quote_amount_exec]&.to_d&.to_s('F'),
      'side' => data[:side].to_s, 'order_type' => data[:order_type].to_s }
  end
  out['bingx_orders'] = orders
  extra = "#{orders.size} orders"
# One case per line: the file stays reviewable in a diff and a third the size of pretty JSON.
lines = out.map { |k, v| v.is_a?(Array) ? "#{k.to_json}: [\n#{v.map { |r| "  #{JSON.generate(r)}" }.join(",\n")}\n]" : "#{k.to_json}: #{JSON.generate(v)}" }
File.write(out_path, "{\n#{lines.join(",\n")}\n}\n")
puts "wrote #{out_path}: #{sizing.size} sizing cases, #{extra}"
