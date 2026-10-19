# Records how a signal bot sizes an order, for rust/tests/signal_sizing.rs (plan 8a T4, r2 §R6):
#   - Bots::Signal::OrderSetter#signal_size (a webhook rule: fixed or percentage, buy of the spendable quote or sell of the free
#     base, a fixed sell capped at the free base),
#   - #api_order_data (an API caller's amount in base or quote, at the price),
#   - Bot::OrderSetter#calculate_best_amount_info on what either produced (a sell forced to :base_and_quote_in_base),
#   - #venue_takes? (whether the caller's denomination reaches the venue as sent),
# on Kraken, Binance, KuCoin and Alpaca, whose market-buy minimum logics differ.
#   bin/rails runner script/rust/record_signal_vectors.rb rust/tests/fixtures/signal_sizing.json
# Nothing is saved and nothing leaves the machine. Re-run (script/rust/repin) when a file under "sources" changes.
require 'json'
require 'digest'

class Unscripted < Exception; end # rubocop:disable Lint/InheritException

Net::HTTP.prepend(Module.new { def connect = raise(Unscripted, "real connection to #{address}") })

out = {
  'sources' => %w[app/models/bots/signal/order_setter.rb app/models/bot/order_setter.rb app/models/bot_signal.rb app/models/ticker.rb
                  app/models/exchanges/kraken.rb app/models/exchanges/binance.rb app/models/exchanges/kucoin.rb app/models/exchanges/alpaca.rb]
    .to_h { |f| [f, Digest::SHA256.file(Rails.root.join(f)).hexdigest] }
}

venues = { 'kraken' => Exchanges::Kraken.new(name: 'Kraken'), 'binance' => Exchanges::Binance.new(name: 'Binance'),
           'kucoin' => Exchanges::Kucoin.new(name: 'KuCoin'), 'alpaca' => Exchanges::Alpaca.new(name: 'Alpaca') }
crypto = Asset.new(symbol: 'BTC', category: 'Cryptocurrency')
tickers = [
  { minimum_base_size: '0.00001', minimum_quote_size: '5', base_decimals: 5, quote_decimals: 8, price_decimals: 2 },
  { minimum_base_size: '0.0001', minimum_quote_size: '0.0001', base_decimals: 4, quote_decimals: 8, price_decimals: 5 },
  { minimum_base_size: '1', minimum_quote_size: '5', base_decimals: 0, quote_decimals: 8, price_decimals: 8 },
  { minimum_base_size: '0.01', minimum_quote_size: '10', base_decimals: 2, quote_decimals: 2, price_decimals: 3 },
  { minimum_base_size: '0.00005', minimum_quote_size: '0.5', base_decimals: 8, quote_decimals: 5, price_decimals: 1 },
  { minimum_base_size: '0.000027', minimum_quote_size: '1', base_decimals: 9, quote_decimals: 2, price_decimals: 2 }
]
rng = Random.new(20_261_019)
decimal = ->(min_exp, max_exp) { (BigDecimal(rng.rand(1...(10**rng.rand(1..9))).to_s) * (BigDecimal(10)**rng.rand(min_exp..max_exp))).to_s('F') }

venues.each do |venue, exchange|
  bot = Bots::Signal.new(exchange:)
  cases = []
  tickers.each_with_index do |t, ti|
    ticker = Ticker.new(exchange:, ticker: 'BTCX', base: 'BTC', quote: 'X', base_asset: crypto, **t.transform_values { |v| v.is_a?(String) ? BigDecimal(v) : v })
    bot.instance_variable_set(:@ticker, ticker)
    # ponytail: 25 per ticker and caller keeps the file at 1,200 cases; the grid covers each venue's floors end to end.
    25.times do
      side = rng.rand(2).zero? ? :buy : :sell
      price = BigDecimal(decimal.(-6, 6))
      percentage = rng.rand(2).zero?
      amount = percentage ? BigDecimal(rng.rand(1..100).to_s) / [1, 10, 4].sample(random: rng) : BigDecimal(decimal.(-6, 5))
      balance = rng.rand(4).zero? ? '0' : decimal.(-8, 6)
      signal = BotSignal.new(direction: side, amount_type: percentage ? :percentage : :fixed, amount:)
      bot.define_singleton_method(:spendable_quote_balance) { BigDecimal(balance) }
      bot.define_singleton_method(:free_base_balance) { BigDecimal(balance) }
      base, quote = bot.send(:signal_size, signal, side, price)
      info = bot.send(:calculate_best_amount_info, { ticker:, price:, amount: base, quote_amount: quote, side:, order_type: :market_order })
      cases << { 'kind' => 'signal', 't' => ti, 'side' => side.to_s, 'amount_type' => signal.amount_type, 'amount' => amount.to_s('F'),
                 'balance' => balance, 'price' => price.to_s('F'), 'base' => base.to_s('F'), 'quote' => quote.to_s('F'),
                 'amount_info' => info.transform_keys(&:to_s).transform_values { |v| v.is_a?(BigDecimal) ? v.to_s('F') : v.to_s } }
    end
    25.times do
      side = rng.rand(2).zero? ? :buy : :sell
      price = BigDecimal(decimal.(-6, 6))
      amount_type = rng.rand(2).zero? ? :base : :quote
      amount = BigDecimal(decimal.(-8, 5))
      # #api_order_data reads the venue's price itself (#signal_price): handed this one instead.
      bot.define_singleton_method(:signal_price) { |_side| price }
      data = bot.send(:api_order_data, side, amount, amount_type)
      info = bot.send(:calculate_best_amount_info, data)
      cases << { 'kind' => 'api', 't' => ti, 'side' => side.to_s, 'amount_type' => amount_type.to_s, 'amount' => amount.to_s('F'),
                 'price' => price.to_s('F'), 'base' => data[:amount].to_s('F'), 'quote' => data[:quote_amount].to_s('F'),
                 'venue_takes' => bot.send(:venue_takes?, side, amount_type),
                 'amount_info' => info.transform_keys(&:to_s).transform_values { |v| v.is_a?(BigDecimal) ? v.to_s('F') : v.to_s } }
    end
  end
  out[venue] = cases
end
out['tickers'] = tickers.map { |t| t.transform_keys(&:to_s).transform_values(&:to_s) }

lines = out.map { |k, v| v.is_a?(Array) ? "#{k.to_json}: [\n#{v.map { |r| "  #{JSON.generate(r)}" }.join(",\n")}\n]" : "#{k.to_json}: #{JSON.generate(v)}" }
File.write(ARGV.fetch(0), "{\n#{lines.join(",\n")}\n}\n")
puts "wrote #{ARGV.fetch(0)}: #{venues.keys.map { |v| "#{out[v].size} #{v}" }.join(', ')} cases"
