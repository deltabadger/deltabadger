# Records a one-asset sell's price, size and order wire on Kraken and Binance for rust/tests/amount.rs (plan 6a T8):
# Bot::OrderSetter#order_price(:sell), the `[base || quote / price, free].min` of
# Bot::Composition::OrderSetter#one_asset_sell_orders_data, #calculate_best_amount_info(side: :sell), then
# Exchange#market_sell / #limit_sell (create_order's sell branch) down to the client call.
#   bin/rails runner script/rust/record_sell_sizing_vectors.rb rust/tests/fixtures/sell_sizing_vectors.json
# Nothing is saved and nothing leaves the machine: each client is replaced by a recorder, and any real connection raises.
# Re-run (script/rust/repin) when a file under "sources" changes; the Rust test fails until then.
require 'json'
require 'digest'

class Unscripted < Exception; end # rubocop:disable Lint/InheritException

Net::HTTP.prepend(Module.new { def connect = raise(Unscripted, "real connection to #{address}") })

out = {
  'sources' => %w[app/models/bot/order_setter.rb app/models/bot/composition/order_setter.rb app/models/bot/limit_orderable.rb
                  app/models/exchange.rb app/models/exchanges/kraken.rb app/models/exchanges/binance.rb app/models/ticker.rb]
    .to_h { |f| [f, Digest::SHA256.file(Rails.root.join(f)).hexdigest] }
}

Rails.configuration.dry_run = false # the test environment trades dry; the real set_*_order is what is recorded
captured = nil
kraken = Exchanges::Kraken.new(name: 'Kraken')
kraken.instance_variable_set(:@client, Object.new.tap do |c|
  c.define_singleton_method(:add_order) { |**kw| captured = kw; Result::Success.new(order_id: 'O-1') }
end)
binance = Exchanges::Binance.new(name: 'Binance')
binance.instance_variable_set(:@client, Object.new.tap do |c|
  c.define_singleton_method(:new_order) { |**kw| captured = kw; Result::Success.new(order_id: "#{kw[:symbol]}-1", raw: {}) }
end)
venues = { 'kraken' => [kraken, 'XXBTZEUR', 'XBT', 'EUR'], 'binance' => [binance, 'BTCUSDT', 'BTC', 'USDT'] }

tickers = [
  { minimum_base_size: '0.00001', minimum_quote_size: '5', base_decimals: 5, quote_decimals: 8, price_decimals: 2 },
  { minimum_base_size: '0.0001', minimum_quote_size: '0.0001', base_decimals: 4, quote_decimals: 8, price_decimals: 5 },
  { minimum_base_size: '1', minimum_quote_size: '5', base_decimals: 0, quote_decimals: 8, price_decimals: 8 },
  { minimum_base_size: '0.001', minimum_quote_size: '5', base_decimals: 3, quote_decimals: 8, price_decimals: 1 },
  { minimum_base_size: '0.01', minimum_quote_size: '10', base_decimals: 2, quote_decimals: 2, price_decimals: 3 },
  { minimum_base_size: '1', minimum_quote_size: '500', base_decimals: 0, quote_decimals: 0, price_decimals: 0 },
  { minimum_base_size: '0.00005', minimum_quote_size: '0.5', base_decimals: 8, quote_decimals: 5, price_decimals: 1 },
  { minimum_base_size: '0.0001', minimum_quote_size: '5', base_decimals: 8, quote_decimals: 2, price_decimals: 2 }
]
# nil: a market sell (priced at the bid); else limit_order_pcnt_distance as the settings store it (a Float).
distances = [nil, 0.0025, 0.01, 0.001, 0.1234]
rng = Random.new(20_261_016)
decimal = ->(min_exp, max_exp) { (BigDecimal(rng.rand(1...(10**rng.rand(1..9))).to_s) * (BigDecimal(10)**rng.rand(min_exp..max_exp))).to_s('F') }

venues.each do |venue, (exchange, pair, base, quote)|
  bots = distances.to_h do |d|
    settings = d ? { 'limit_ordered' => true, 'limit_order_pcnt_distance' => d } : {}
    [d, Bots::DcaMultiAsset.new(exchange:, settings:)]
  end
  cases = []
  tickers.each_with_index do |t, ti|
    ticker = Ticker.new(exchange:, ticker: pair, base:, quote:, **t.transform_values { |v| v.is_a?(String) ? BigDecimal(v) : v })
    1_520.times do
      d = distances.sample(random: rng)
      bot = bots[d]
      reference = decimal.(-9, 6)
      price = bot.send(:order_price, ticker, :sell, BigDecimal(reference))
      next if price.zero? # a zero price fails the tick before sizing (ZeroDivisionError / "limit price rounds to zero")

      denomination = rng.rand(2).zero? ? 'base' : 'quote'
      wanted_s = denomination == 'base' ? decimal.(-9, 4) : decimal.(-5, 5)
      wanted = denomination == 'base' ? BigDecimal(wanted_s) : BigDecimal(wanted_s) / price
      free_s = rng.rand(3).zero? ? decimal.(-9, 4) : (wanted * 10).to_s('F') # a third of the wallets hold less than wanted
      order = bot.send(:sell_order_data, ticker, price, [wanted, BigDecimal(free_s)].min)
      info = bot.send(:calculate_best_amount_info, order)
      raise "#{venue}: a sell sized in #{info[:amount_type]}" unless info[:amount_type] == :base

      captured = nil
      # Bot::OrderSetter#create_order's sell branch, at the exchange (Bot::ExchangeUser's guard needs a saved bot).
      placed = if d
                 exchange.limit_sell(ticker:, amount: info[:amount], amount_type: info[:amount_type], price:)
               else
                 exchange.market_sell(ticker:, amount: info[:amount], amount_type: info[:amount_type])
               end
      row = { 't' => ti, 'd' => d, 'reference' => reference, 'denomination' => denomination, 'wanted' => wanted_s, 'free' => free_s,
              'price' => price.to_s('F'), 'amount' => order[:amount].to_s('F'), 'quote_amount' => order[:quote_amount].to_s('F'),
              'below_minimum' => info[:below_minimum_amount] }
      row['wire'] = captured&.compact&.transform_keys(&:to_s)&.transform_values { |v| v.is_a?(Array) ? v : v.to_s }
      row['refused'] = placed.errors.join if captured.nil?
      cases << row
    end
  end
  out["#{venue}_sell"] = cases
end
out['tickers'] = tickers.map { |t| t.transform_keys(&:to_s).transform_values(&:to_s) }

# One case per line: the file stays reviewable in a diff.
lines = out.map { |k, v| v.is_a?(Array) ? "#{k.to_json}: [\n#{v.map { |r| "  #{JSON.generate(r)}" }.join(",\n")}\n]" : "#{k.to_json}: #{JSON.generate(v)}" }
File.write(ARGV.fetch(0), "{\n#{lines.join(",\n")}\n}\n")
puts "wrote #{ARGV.fetch(0)}: #{out['kraken_sell'].size} Kraken and #{out['binance_sell'].size} Binance sell cases"
