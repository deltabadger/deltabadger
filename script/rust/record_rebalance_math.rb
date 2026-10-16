# Records threshold rebalancing's valuation and ranking for rust/tests/rebalance_math.rs (plan 6b T2):
# Bot::Composition::Measurable#metrics_with_current_prices (Alpaca's latest trades, Float or String prices, through
# Exchanges::Alpaca#get_crypto_tickers_prices), Bot::Composition::Rebalancer#composition_targets / #normalise_targets /
# #unpriced_holding?, Bot::Rebalanceable#rebalance_drift / #rebalance_due?, Bot::Rebalancer#extreme_rebalance_entry, and
# get_sell_orders_data's ranking (rebalance_deviation over tradeable_rebalance_entries, sort_by.with_index).
#   bin/rails runner script/rust/record_rebalance_math.rb rust/tests/fixtures/rebalance_math.json
# The ledger walk (`metrics`) is stubbed with the case's holdings; everything after it is Rails' own code, forced, as the
# trade paths read it since #600. Nothing is saved (each case rolls back) and nothing leaves the machine.
# Re-run (script/rust/repin) when a file under "sources" changes; the Rust test fails until then.
require 'json'
require 'digest'

class Unscripted < Exception; end # rubocop:disable Lint/InheritException

Net::HTTP.prepend(Module.new { def connect = raise(Unscripted, "real connection to #{address}") })

SOURCES = %w[app/models/bot/composition/rebalancer.rb app/models/bot/rebalancer.rb app/models/bot/rebalanceable.rb
             app/models/bot/composition/measurable.rb app/models/bot/composition/allocatable.rb app/models/bot/composition/order_setter.rb
             app/models/bots/dca_multi_asset.rb app/models/bots/dca_multi_asset/allocatable.rb app/models/exchanges/alpaca.rb].freeze
SYMBOLS = %w[BTC ETH SOL DOGE XRP ADA].freeze
CASES = 1_200

out = { 'sources' => SOURCES.to_h { |f| [f, Digest::SHA256.file(Rails.root.join(f)).hexdigest] }, 'cases' => [] }
rng = Random.new(20_261_016)
pick = ->(weights) { r = rng.rand * weights.values.sum; weights.find { |_, w| (r -= w).negative? }&.first || weights.keys.last }
decimal = ->(min_exp, max_exp) { BigDecimal(rng.rand(1...(10**rng.rand(1..9))).to_s) * (BigDecimal(10)**rng.rand(min_exp..max_exp)) }

# One case: members [{ 'sym', 'target' (what is written), 'in_index', 'tradeable' }], holdings { sym => amount String },
# prices { sym => Float | String } (a symbol missing has no trade), locked [sym], withdrawal String, settings overrides.
def crafted
  m = ->(sym, target, in_index: true, tradeable: true) { { 'sym' => sym, 'target' => target, 'in_index' => in_index, 'tradeable' => tradeable } }
  [
    # Exactly at the band: 55/45 against 50/50 is a drift of exactly 0.05, which is not due (strictly greater).
    { 'members' => [m.('BTC', 0.5), m.('ETH', 0.5)], 'holdings' => { 'BTC' => '55', 'ETH' => '45' }, 'prices' => { 'BTC' => '1', 'ETH' => '1' } },
    { 'members' => [m.('BTC', 0.5), m.('ETH', 0.5)], 'holdings' => { 'BTC' => '55.0000001', 'ETH' => '44.9999999' }, 'prices' => { 'BTC' => '1', 'ETH' => '1' } },
    { 'members' => [m.('BTC', 0.5), m.('ETH', 0.5)], 'holdings' => { 'BTC' => '57', 'ETH' => '43' }, 'prices' => { 'BTC' => 1.0, 'ETH' => 1.0 },
      'settings' => { 'rebalance_threshold' => 0.07 } },
    { 'members' => [m.('BTC', 0.5), m.('ETH', 0.5)], 'holdings' => { 'BTC' => '57', 'ETH' => '43' }, 'prices' => { 'BTC' => 1.0, 'ETH' => 1.0 },
      'settings' => { 'rebalance_threshold' => '0.06' } },
    { 'members' => [m.('BTC', 0.5), m.('ETH', 0.5)], 'holdings' => { 'BTC' => '57', 'ETH' => '43' }, 'prices' => { 'BTC' => 1.0, 'ETH' => 1.0 },
      'settings' => { 'rebalance_threshold' => '' } },
    # Ties: equal deviations, the smaller target first, then composition order.
    { 'members' => [m.('BTC', 0.25), m.('ETH', 0.25), m.('SOL', 0.5)], 'holdings' => { 'BTC' => '30', 'ETH' => '30', 'SOL' => '40' },
      'prices' => { 'BTC' => '1', 'ETH' => '1', 'SOL' => '1' } },
    { 'members' => [m.('SOL', 0.5), m.('BTC', 0.2), m.('ETH', 0.3)], 'holdings' => { 'BTC' => '30', 'ETH' => '40', 'SOL' => '30' },
      'prices' => { 'BTC' => '1', 'ETH' => '1', 'SOL' => '1' } },
    { 'members' => [m.('BTC', 0.333333), m.('ETH', 0.333333), m.('SOL', 0.333333)], 'holdings' => { 'BTC' => '1', 'ETH' => '1', 'SOL' => '1' },
      'prices' => { 'BTC' => 3.0, 'ETH' => 3.0, 'SOL' => 3.0 } },
    # Unknown weights keep their own share; recorded ones fill the rest. All unknown; every survivor at zero.
    { 'members' => [m.('BTC', nil), m.('ETH', 0.6), m.('SOL', 0.2)], 'holdings' => { 'BTC' => '2', 'ETH' => '1', 'SOL' => '7' },
      'prices' => { 'BTC' => '10', 'ETH' => '10', 'SOL' => '10' } },
    { 'members' => [m.('BTC', nil), m.('ETH', nil)], 'holdings' => { 'BTC' => '2', 'ETH' => '1' }, 'prices' => { 'BTC' => '10', 'ETH' => '10' } },
    { 'members' => [m.('BTC', 0), m.('ETH', 0)], 'holdings' => { 'BTC' => '2', 'ETH' => '1' }, 'prices' => { 'BTC' => '10', 'ETH' => '10' } },
    # A member's holding with no trade: nothing to steer by. A member with no ticker: it is not asked for.
    { 'members' => [m.('BTC', 0.5), m.('ETH', 0.5)], 'holdings' => { 'BTC' => '2', 'ETH' => '1' }, 'prices' => { 'BTC' => '10' } },
    { 'members' => [m.('BTC', 0.5), m.('ETH', 0.5, tradeable: false)], 'holdings' => { 'BTC' => '2', 'ETH' => '1' }, 'prices' => { 'BTC' => '10' } },
    # An exited holding is out of the denominator; a locked member sits out.
    { 'members' => [m.('BTC', 0.5), m.('ETH', 0.5), m.('SOL', 0.2, in_index: false)], 'holdings' => { 'BTC' => '2', 'ETH' => '1', 'SOL' => '50' },
      'prices' => { 'BTC' => '10', 'ETH' => '10', 'SOL' => '10' } },
    { 'members' => [m.('BTC', 0.5), m.('ETH', 0.3), m.('SOL', 0.2)], 'holdings' => { 'BTC' => '2', 'ETH' => '1', 'SOL' => '5' },
      'prices' => { 'BTC' => '10', 'ETH' => '10', 'SOL' => '10' }, 'locked' => ['SOL'] },
    # Nothing held: no drift, no sell, a buy still has a target. A withdrawal larger than the portfolio.
    { 'members' => [m.('BTC', 0.7), m.('ETH', 0.3)], 'holdings' => {}, 'prices' => { 'BTC' => '10', 'ETH' => '10' } },
    { 'members' => [m.('BTC', 0.7), m.('ETH', 0.3)], 'holdings' => { 'BTC' => '1', 'ETH' => '1' }, 'prices' => { 'BTC' => '10', 'ETH' => '10' },
      'withdrawal' => '1000' },
    # Seventeen-digit Float prices: Alpaca's JSON number read as BigDecimal(value.to_s).
    { 'members' => [m.('BTC', 0.5), m.('ETH', 0.5)], 'holdings' => { 'BTC' => '0.123456789', 'ETH' => '3.3' },
      'prices' => { 'BTC' => 64_123.456789012345, 'ETH' => 0.12345678901234568 } },
    { 'members' => [m.('BTC', 0.5), m.('ETH', 0.5)], 'holdings' => { 'BTC' => '1', 'ETH' => '1' }, 'prices' => { 'BTC' => 1.0e-05, 'ETH' => 1.0e+16 } },
    { 'members' => [m.('BTC', 0.5), m.('ETH', 0.5)], 'holdings' => { 'BTC' => '55', 'ETH' => '45' }, 'prices' => { 'BTC' => '1', 'ETH' => '1' },
      'settings' => { 'rebalance_enabled' => false } }
  ]
end

def random_case(rng, pick, decimal)
  n = rng.rand(1..5)
  syms = SYMBOLS.shuffle(random: rng).first(n + (rng.rand(4).zero? ? 1 : 0))
  members = syms.map.with_index do |sym, i|
    target = case pick.({ unknown: 1, zero: 1, equal: 3, tie: 2, random: 6 })
             when :unknown then nil
             when :zero then 0
             when :equal then 1.0 / n
             when :tie then [0.25, 0.5, 0.2][i % 3]
             else rng.rand.round(rng.rand(2..9))
             end
    { 'sym' => sym, 'target' => target, 'in_index' => i < n, 'tradeable' => rng.rand(12).positive? }
  end
  holdings = {}
  prices = {}
  syms.each do |sym|
    case pick.({ none: 2, zero: 1, held: 12 })
    when :zero then holdings[sym] = '0'
    when :held then holdings[sym] = decimal.(-9, 3).to_s('F')
    end
    next if rng.rand(25).zero? # no trade for it

    prices[sym] = case pick.({ float: 3, short_float: 1, string: 3 })
                  when :float then rng.rand * (10**rng.rand(-3..5))
                  when :short_float then (rng.rand * 1000).round(rng.rand(0..4))
                  else decimal.(-6, 2).to_s('F')
                  end
  end
  # Equal holdings at equal prices make ties in value too.
  if rng.rand(8).zero? && syms.size > 1
    syms.each { |sym| holdings[sym] = '1.5' }
    syms.each { |sym| prices[sym] = '2' }
  end
  settings = {}
  settings['rebalance_threshold'] = pick.({ nil => 2, 0.05 => 1, 0.1 => 1, 0.02 => 1, 0.0001 => 1, 'string' => 1 })
  settings['rebalance_threshold'] = decimal.(-4, -1).to_s('F') if settings['rebalance_threshold'] == 'string'
  settings['rebalance_enabled'] = rng.rand(10).positive?
  locked = rng.rand(10).zero? ? syms.sample(rng.rand(1..syms.size), random: rng) : []
  { 'members' => members, 'holdings' => holdings, 'prices' => prices, 'settings' => settings, 'locked' => locked,
    'withdrawal' => decimal.(-3, 3).to_s('F') }
end

ActiveRecord::Base.transaction do
  user = User.new(name: 'Owner', email: 'owner@example.com', encrypted_password: 'x', confirmed_at: Time.current, setup_completed: true)
  user.save!(validate: false)
  alpaca = Exchanges::Alpaca.create!(name: 'Alpaca', maker_fee: '0.15', taker_fee: '0.25')
  usd = Asset.create!(external_id: 'usd', symbol: 'USD', name: 'US Dollar', category: 'Currency')
  assets = SYMBOLS.to_h { |sym| [sym, Asset.create!(external_id: sym.downcase, symbol: sym, name: sym, category: 'Cryptocurrency')] }
  [usd, *assets.values].each { |a| ExchangeAsset.create!(exchange: alpaca, asset: a, available: true) }
  tickers = assets.transform_values do |asset|
    Ticker.create!(exchange: alpaca, ticker: "#{asset.symbol}/USD", base: asset.symbol, quote: 'USD', base_asset: asset, quote_asset: usd,
                   base_decimals: 9, quote_decimals: 2, price_decimals: 9, minimum_base_size: '0.000000001', minimum_quote_size: '1',
                   available: true, trading_enabled: true)
  end
  ApiKey.new(user:, exchange: alpaca, key: 'k', secret: 's', passphrase: 'paper', status: :correct, key_type: :trading).save!(validate: false)

  (crafted + Array.new(CASES) { random_case(rng, pick, decimal) }).each do |kase|
    expected = nil
    ActiveRecord::Base.transaction(requires_new: true) do
      members = kase['members']
      in_index = members.select { |mm| mm['in_index'] }
      members.reject { |mm| mm['tradeable'] }.each { |mm| tickers[mm['sym']].update_columns(available: false) }
      allocations = in_index.to_h { |mm| [assets[mm['sym']].id.to_s, 1.0 / in_index.size] }
      bot = Bots::DcaMultiAsset.new(user:, exchange: alpaca, settings: {
        'quote_asset_id' => usd.id, 'quote_amount' => 60.0, 'interval' => 'week', 'allocations' => allocations,
        'base_asset_ids' => in_index.map { |mm| assets[mm['sym']].id }, 'rebalance_enabled' => true
      }.merge(kase['settings'] || {}).compact)
      bot.set_missed_quote_amount
      bot.save!(validate: false)
      BotIndexAsset.where(bot_id: bot.id).delete_all # the case's composition, not the one saving derived
      bot.bot_index_assets.reset
      raw = members.map do |mm|
        bia = BotIndexAsset.create!(bot:, asset: assets[mm['sym']], ticker: tickers[mm['sym']], target_allocation: mm['target'],
                                    in_index: mm['in_index'], entered_at: Time.current)
        BotIndexAsset.connection.select_value("SELECT target_allocation FROM bot_index_assets WHERE id = #{bia.id}")
      end
      walk = bot.send(:initialize_metrics_data)
      walk[:chart][:labels] << 1.day.ago
      kase['holdings'].each do |sym, amount|
        walk[:asset_breakdown][sym] = { amount: BigDecimal(amount), quote_invested: BigDecimal('1'), tax_units: BigDecimal(amount),
                                        tax_basis: BigDecimal('1'), tax_cost_unknown: false }
        walk[:key_assets][sym] = assets[sym].id
      end
      bot.define_singleton_method(:metrics) { |force: false| walk } # rubocop:disable Lint/UnusedBlockArgument
      locked = kase['locked'].to_a.to_set { |sym| assets[sym].id }
      bot.define_singleton_method(:locked_asset_ids) { |*| locked }
      trades = kase['prices'].to_h { |sym, p| ["#{sym}/USD", { 'p' => p }] }
      client = Object.new
      client.define_singleton_method(:get_crypto_latest_trade) { |symbols:| Result::Success.new('trades' => trades.slice(*symbols)) }
      bot.exchange.define_singleton_method(:market_data_client) { client }

      data = bot.metrics_with_current_prices(force: true)
      entries = bot.send(:composition_targets, locked:, force: true)
      row = ->(e) { e && { 'asset_id' => e[:asset_id], 'symbol' => e[:symbol], 'amount' => e[:amount].to_s('F'), 'value' => e[:value].to_s('F'), 'target' => e[:target].to_d.to_s('F') } }
      extreme = ->(dir) { e, total = bot.send(:extreme_rebalance_entry, dir); [e && e[:asset_id], total.to_d.to_s('F')] }
      ranking = if entries
                  withdrawal = BigDecimal(kase['withdrawal'] || '0')
                  deviation = bot.send(:rebalance_deviation, [entries.sum { |e| e[:value].to_d } - withdrawal, 0].max)
                  bot.send(:tradeable_rebalance_entries, entries).sort_by.with_index { |e, i| over, tie = deviation.call(e); [-over, -tie, i] }.map { |e| e[:asset_id] }
                end
      expected = {
        'stale' => data[:prices_stale] == true,
        'targets' => entries&.map(&row),
        'drift' => bot.rebalance_drift&.to_s('F'),
        'due' => bot.rebalance_due?,
        'max' => extreme.(:max), 'min' => extreme.(:min),
        'ranking' => ranking
      }
      kase = kase.merge('members' => members.zip(raw).map { |mm, r| mm.merge('asset_id' => assets[mm['sym']].id, 'raw' => r) },
                        'asset_ids' => assets.transform_values(&:id), 'settings' => bot.settings.slice('rebalance_enabled', 'rebalance_threshold'))
      raise ActiveRecord::Rollback
    end
    # A Float price goes out as its Float#to_s: Rails' JSON encoder (Oj) writes a Float with 16 digits, which is another
    # double than the 17-digit one Alpaca sent and Rails read.
    prices = kase['prices'].transform_values { |p| p.is_a?(Float) ? { 'float' => p.to_s } : p }
    out['cases'] << kase.merge('prices' => prices, 'expected' => expected)
  end
  raise ActiveRecord::Rollback
end

File.write(ARGV.fetch(0), "{\"sources\": #{JSON.pretty_generate(out['sources'])},\n\"cases\": [\n#{out['cases'].map { |c| JSON.generate(c) }.join(",\n")}\n]}\n")
puts "wrote #{out['cases'].size} cases to #{ARGV.fetch(0)}"
