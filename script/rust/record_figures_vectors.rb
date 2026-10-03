# Records what rust/src/figures must reproduce, from Ruby and from the app's own code.
#   bin/rails runner script/rust/record_figures_vectors.rb rust/tests/fixtures/figures_vectors.json
# The numbers, times and JSON come from Ruby itself; the rest from the app's own methods, each called as it stands.
# Every input is fixed (a seeded generator), so two runs write the same file. Re-run when Ruby, bigdecimal, oj or
# ActiveSupport move, or when one of the files under "ported_sources" changes.
require 'json'
require 'digest'

rng = Random.new(20_261_002)
bits = ->(float) { [float].pack('G').unpack1('H*') }
tag = lambda do |value|
  case value
  when Integer then { 'i' => value }
  when BigDecimal then { 'd' => value.to_s }
  when Float then { 'f' => bits.(value) }
  else value
  end
end

floats = [0.0, -0.0, 1.0, -1.0, 100.0, 0.1, 0.1 + 0.2, 100.0 / 3, 1.0e-5, 1.0e-4, 0.00012345, 1.0e15, 1.0e16, 1.0e17, 1.0e22, 5.0e-324,
          123_456_789.0, 9_007_199_254_740_993.0, 0.5, 2.5, -2.5, 1234.5678, 59.99999999604, 0.30000000000000004, 4.35, 1.0e-7,
          9.999999999999999e22, 1.7976931348623157e308, 123_456.789e3, 1.0 / 3, 2.0 / 3, 201.609360744018237, 0.000100000000000001] +
         Array.new(600) { (rng.rand * (10.0**rng.rand(-9..17))).round(rng.rand(0..17)).to_f * (rng.rand < 0.2 ? -1 : 1) } +
         Array.new(200) { rng.rand(1..10**9).to_f / (10**rng.rand(0..9)) } + Array.new(200) { rng.rand } +
         # sixteen significant digits ending in 0001 or 9999: where Oj falls back to Float#to_s
         Array.new(200) { |i| "#{rng.rand(1..9)}.#{format('%011d', rng.rand(10**11))}#{i.even? ? '0001' : '9999'}e#{rng.rand(-8..12)}".to_f }

# BigDecimal has a negative zero, and prints it: it is an operand like any other, and so is the Float one.
decimals = %w[0 -0 1 -1 0.5 1.5 100 59.99999999604 0.591891092 101.37 0.000000001 123456789.123456789 3 7 0.1 -0.25 199.99999997864 1e-12 2.675 -2.675 0.005 -0.001]
           .map { |text| BigDecimal(text) }
operands = [0, 1, -1, 3, 7, 100] + decimals + [0.0, -0.0, 1.0, 0.1, 2.5, -2.5, 1.0e-5, 65_000.5, 1.08, 0.1 + 0.2, 100.0 / 3, 1234.5678901234567]
num = operands.product(operands).flat_map do |a, b|
  %i[+ - * / <=> min max].map do |op|
    result = begin
      case op
      when :min then [a, b].min
      when :max then [a, b].max
      else a.public_send(op, b)
      end
    rescue ZeroDivisionError, FloatDomainError => e
      { 'raise' => e.class.name }
    end
    # Infinity and NaN are not figures, whichever way Ruby arrives at them (1 / 0.to_d is Infinity, 1.to_d / 0 raises).
    result = { 'raise' => 'not a number' } if result.is_a?(Hash) || ((result.is_a?(Float) || result.is_a?(BigDecimal)) && !result.finite?)
    [op.to_s, tag.(a), tag.(b), tag.(result)]
  end
end

# String#to_d, which a venue's price and a provider's rate go through: it reads a number off the front of anything.
to_d = ['104.52', '0.9993', ' 12 ', "\n1.5\n", '12abc', 'abc', '', '1_000.5', '1__0', '_1', '1_', '1e3', '1E3', '1e', '1e+2', '1e-2', '1d3', '1.5e3x', '0x1A', '.5',
        '5.', '-.5', '+7', '-7.25', '1,5', '1.2.3', '-0', '-0.0', '+0', '0.0', '00012.500', '1 2', '--1', '+-1', '1e1_0', '1_0e1', '9' * 40, "0.#{'0' * 39}1",
        'NaN', 'Infinity', '-Infinity', '+Infinity', 'nan', 'inf', 'NaNx', 'Infinityx', ' NaN', ' Infinity', '-NaN']
       .map { |text| [text, text.to_d.then { |d| d.finite? ? d.to_s : 'not finite' }] }

times = [Time.utc(2026, 3, 2, 14, 30, 0), Time.utc(2026, 3, 2, 14, 30, 0, 250_000), Time.utc(2026, 3, 2, 14, 30, 1),
         Time.utc(2025, 1, 1, 0, 0, 0, 1), Time.utc(2021, 5, 17, 9, 0, 0, 123_456), Time.at(1_772_461_800, 123_456_789, :nsec).utc,
         Time.utc(2026, 9, 20, 23, 59, 59, 999_999), Time.utc(2016, 1, 4, 5, 0, 0)]
nanos = ->(time) { (time.to_i * 1_000_000_000) + time.nsec }

# ---- the app's own pure functions, on seeded inputs ---------------------------------------------------------

money = ->(low, high, places) { BigDecimal(rng.rand(low..high).round(places).to_s) }
at_nanos = ->(seconds) { 1_772_461_800_000_000_000 + (seconds * 1_000_000_000).to_i }
moment = ->(nanos_since_epoch) { Time.at(nanos_since_epoch / 1_000_000_000, nanos_since_epoch % 1_000_000_000, :nsec).utc }

holding_keys = Array.new(200) do
  identities = (Array.new(rng.rand(1..5)) { rng.rand(1..40) } + Array.new(rng.rand(0..4)) { %w[POR por ABC XYZ POR#?].sample(random: rng) }).uniq.shuffle(random: rng)
  candidates = identities.map { |identity| [identity, identity.is_a?(Integer) ? %w[POR ABC XYZ POR#? QQQ].sample(random: rng) : identity] }
  [candidates, Bot::Composition::HoldingKeys.call(candidates.to_h).to_a]
end

lot_json = ->(lots) { lots.map { |lot| [lot[:amount].to_s, lot[:cost]&.to_s] } }
random_lots = -> { Array.new(rng.rand(0..5)) { { amount: money.(0.001, 3.0, 9), cost: rng.rand < 0.2 ? nil : money.(1.0, 400.0, 6) } } }
tax_lots = Array.new(300) do
  lots = random_lots.()
  amount = [money.(0.001, 8.0, 9), lots.first&.dig(:amount), lots.sum { |lot| lot[:amount] }, BigDecimal('0')].compact.sample(random: rng)
  proceeds = money.(0.0, 900.0, 6)
  factor = [BigDecimal('4'), BigDecimal('0.1'), BigDecimal('3') / 2, BigDecimal('1') / 3].sample(random: rng)
  consumed = lots.map(&:dup).tap { |copy| Bot::TaxLots.consume(copy, amount) }
  restated = lots.map(&:dup).tap { |copy| Bot::TaxLots.split!(copy, factor) }
  { 'lots' => lot_json.(lots), 'amount' => amount.to_s, 'proceeds' => proceeds.to_s, 'factor' => factor.to_s,
    'basis' => tag.(Bot::TaxLots.basis(lots)), 'units' => tag.(lots.sum { |lot| lot[:amount] }), 'unknown' => Bot::TaxLots.unknown_cost?(lots),
    'cost_of' => Bot::TaxLots.cost_of(lots, amount).to_s, 'loss_in' => Bot::TaxLots.loss_in?(lots, amount, proceeds),
    'consumed' => lot_json.(consumed), 'split' => lot_json.(restated) }
end

accounting = Object.new.extend(Bot::RebalanceAccounting)
books_json = ->(books, ledger) { [books.transform_values(&tag).stringify_keys, ledger.map { |key, entry| [key, tag.(entry[:amount]), tag.(entry[:invested])] }] }
books = Array.new(250) do
  ledger = Hash.new { |hash, key| hash[key] = { amount: 0, invested: 0 } }
  state = accounting.new_rebalance_books
  steps = Array.new(rng.rand(1..14)) do
    side = rng.rand < 0.4 ? 'sell' : 'buy'
    type = %w[REGULAR REGULAR REBALANCE LIQUIDATION REDEPLOY ODD].sample(random: rng)
    key = %w[AAA BBB CCC].sample(random: rng)
    amount = money.(0.001, 2.0, 9)
    quote = money.(0.01, 300.0, 6)
    unpriced = side == 'sell' && rng.rand < 0.15
    branch = if unpriced
               accounting.apply_unpriced_sell(ledger, state, key:, amount_exec: amount, quote_amount_exec: quote)
               'unpriced_sell'
             else
               accounting.apply_fill(ledger, state, key:, side:, transaction_type: type, amount_exec: amount, quote_amount_exec: quote).to_s
             end
    [side, type, key, amount.to_s, quote.to_s, unpriced, branch, tag.(accounting.uninvested_cash(state)), *books_json.(state, ledger)]
  end
  steps
end

confirmed = [nil, 'unknown', 'open', 'closed', 'cancelled', 'abandoned'].product([nil, '100.5'], [nil, '0.5'], [nil, '0', '0.25'], [nil, '0', '25.1']).map do |status, *numbers|
  price, amount, amount_exec, quote_amount_exec = numbers.map { |n| n && BigDecimal(n) }
  [status, *numbers, *Transaction.confirmed_exec_amounts(status, price, amount, amount_exec, quote_amount_exec).map { |n| n&.to_s }]
end

restatable = Object.new.extend(Bot::Restatable)
ratios = ['10:1', '1:10', '3:2', '4:0', '0:4', 'a:b', '10', '10:1:2', '1.5:1', ' 2:1', '2:1 ', '', nil, 5, 2.5, '2 : 1', '07:1', '1e1:1', '2.:1', '.5:1', '2:1:', ':2', '2:',
          '2:1::', "2:1\n", '٣:1', '-2:1', '+2:1', '1:3', '2:3', '1000000:1', '0.0:1', '1:0.000', true, ['2:1'], { 'a' => 1 }]
split_factor = ratios.map { |ratio| [ratio, restatable.send(:split_factor, Struct.new(:raw_data).new({ 'split_ratio' => ratio }))&.to_s] }

measurable = Object.new.extend(Bot::Composition::Measurable)
durations = [0, 1, 17_999.999, 18_000, 18_000.001, 89_999.9, 90_000, 269_999, 270_000, 539_999.5, 540_000, 1_079_999, 1_080_000, 1_080_001, 86_400 * 400, -5, 0.25]
timeframes = durations.map { |seconds| [bits.(seconds.to_f), measurable.send(:optimal_candles_timeframe_for_duration, seconds.to_f).to_i] }
pnl = [[0, 0], [0, BigDecimal('5')], [BigDecimal('0'), BigDecimal('5')], [BigDecimal('199.99999997864'), BigDecimal('201.64698049488')], [BigDecimal('3'), BigDecimal('1')],
       [BigDecimal('100'), 0], [7, BigDecimal('7.7')], [BigDecimal('0.000001'), BigDecimal('123456.789')]]
      .map { |from, to| [tag.(from), tag.(to), tag.(measurable.send(:calculate_pnl, from, to))] }

chart = Object.new.extend(Bot::ChartSeries)
random_marks = lambda do |count, span|
  stamps = Array.new(count) { at_nanos.(rng.rand(0..span) + (rng.rand < 0.3 ? rng.rand(999_999_999) / 1e9 : 0)) }.uniq.sort
  stamps.map { |nanos_at| [nanos_at, money.(0.5, 500.0, 4)] }
end
grid_price = Array.new(120) do
  marks = random_marks.(rng.rand(0..7), 86_400)
  asked = (marks.map(&:first) + Array.new(4) { at_nanos.(rng.rand(-3600..90_000) + (rng.rand(999) / 1000r)) }).uniq
  [marks.map { |nanos_at, price| [nanos_at, price.to_s] },
   asked.map { |nanos_at| [nanos_at, chart.send(:chart_grid_price, marks.map { |t, price| [moment.(t), price] }, moment.(nanos_at))&.to_s] }]
end

thinned = [5, 500, 501, 760, 1300].flat_map { |count| [3600, 86_400 * 90] .map { |span| [count, span] } }.push([600, 0]).map do |count, span|
  stamps = Array.new(count) { at_nanos.(span.zero? ? 0 : rng.rand(0..span) + (rng.rand(999_999) / 1_000_000r)) }.sort
  marks = stamps.map { |nanos_at| [nanos_at, %w[AAA BBB CCC].sample(random: rng), money.(0.001, 2.0, 9), money.(1.0, 90.0, 6), 1] }
  out = chart.chart_thinned_marks(marks.map { |nanos_at, *rest| [moment.(nanos_at), *rest] })
  [marks.map { |nanos_at, key, amount, quote, fills| [nanos_at, key, amount.to_s, quote.to_s, fills] },
   out.map { |time, key, amount, quote, fills| [nanos.(time), key, amount.to_s, quote.to_s, fills] }]
end

# #chart_marked_at_market on small random charts: holdings and cost per point, grids that cover them or do not.
row_json = ->(row) { row.map { |key, value| [key, tag.(value)] } }
holes = Random.new(7) # its own generator: the vectors recorded after these do not move when a hole does
marked = Array.new(60) do
  keys = %w[AAA BBB CCC].first(rng.rand(1..3))
  labels = Array.new(rng.rand(1..6)) { at_nanos.(rng.rand(0..40_000)) }.sort
  labels[-1] = labels[-2] if labels.size > 1 && rng.rand < 0.2 # two fills in one moment
  held = {}
  rows = labels.map do
    key = keys.sample(random: rng)
    held = held.merge(key => rng.rand < 0.15 ? 0 : money.(0.0, 3.0, 6))
    held
  end
  cost = rows.map { |row| row.transform_values { |amount| amount.zero? ? 0 : money.(1.0, 300.0, 4) } }
  # A point with no row of its own (a hash cached before the rows existed has such points): it reads the row before.
  rows = rows.each_with_index.map { |row, i| i.positive? && holes.rand < 0.15 ? {} : row }
  cash = labels.map { rng.rand < 0.6 ? 0 : money.(0.0, 50.0, 4) }
  values = labels.map { money.(10.0, 900.0, 4) }
  invested = labels.map { money.(10.0, 900.0, 4) }
  grids = keys.select { rng.rand < 0.85 }.to_h { |key| [key, random_marks.(rng.rand(1..9), 45_000).map { |t, price| [t - (rng.rand < 0.5 ? 3_600_000_000_000 : 0), price] }.sort_by(&:first)] }
  display = grids.merge(keys.select { rng.rand < 0.3 }.to_h { |key| [key, random_marks.(rng.rand(1..6), 45_000)] })
  priceable = keys.select { rng.rand < 0.9 }
  to_time = ->(grid) { grid.transform_values { |marks| marks.map { |t, price| [moment.(t), price] } } }
  input = { labels: labels.map(&moment), series: [values, invested], extra_series: rows, invested_series: cost, cash_series: cash }
  out = chart.chart_marked_at_market(input, to_time.(grids), display_grids: to_time.(display),
                                     holdings: ->(i) { (rows[i] || {}).slice(*priceable) }, basis: ->(i) { (cost[i] || {}).slice(*priceable) },
                                     cash: ->(i) { cash[i] || 0 })
  grid_json = ->(grid) { grid.map { |key, marks| [key, marks.map { |t, price| [t, price.to_s] }] } }
  { 'labels' => labels, 'values' => values.map(&tag), 'invested' => invested.map(&tag), 'extra' => rows.map(&row_json), 'cost' => cost.map(&row_json),
    'cash' => cash.map(&tag), 'grids' => grid_json.(grids), 'display' => grid_json.(display), 'priceable' => priceable,
    'out' => { 'labels' => out[:labels].map(&nanos), 'values' => out[:series][0].map(&tag), 'invested' => out[:series][1].map(&tag),
               'prices' => out[:prices].map { |key, serie| [key, serie.map { |price| price&.to_s }] },
               'assets' => out[:assets].map { |key, serie| [key, serie[:value].map { |v| v && tag.(v) }, serie[:invested].map(&tag)] } } }
end

history = User::PnlHistory.new(nil)
merged = (Array.new(50) { [rng.rand(1..3), rng.rand(1..9)] } + [[2, 120], [1, 1], [0, 0]]).map do |bots, points|
  tracks = Array.new(bots) do
    stamps = Array.new(rng.rand(1..points)) { at_nanos.(rng.rand(0..(86_400 * 30)) + (rng.rand < 0.5 ? rng.rand(999_999) / 1_000_000r : 0)) }.uniq.sort
    stamps.map { |nanos_at| [nanos_at, money.(0.0, 900.0, 4), money.(0.0, 900.0, 4)] }
  end
  out = history.send(:merge, tracks.map { |track| track.map { |nanos_at, value, invested| [moment.(nanos_at), value, invested] } })
  [tracks.map { |track| track.map { |nanos_at, value, invested| [nanos_at, value.to_s, invested.to_s] } },
   out && { 'percent' => out[:percent].map(&bits), 'profit_usd' => out[:profit_usd].map(&bits), 'at' => out[:at], 'days' => bits.(out[:days]) }]
end

# What this library mirrors. A change to any of these files fails rust/tests/figures_vectors.rs until the port is
# checked against it and this file is recorded again.
PORTED = %w[
  app/models/bot/composition/measurable.rb app/models/bot/rebalance_accounting.rb app/models/bot/tax_lots.rb app/models/bot/restatable.rb
  app/models/bot/composition/holding_keys.rb app/models/bot/chart_series.rb app/models/bot/composition/allocatable.rb
  app/models/candle_series_cache.rb app/models/user/pnl_history.rb app/models/utilities/currency.rb app/models/denomination.rb
  app/views/bots/_chart.html.erb
].freeze

vectors = {
  'float_to_s' => floats.map { |f| [bits.(f), f.to_s] },
  # Through the encoder Rails uses for every data attribute and every `to_json` (Oj, config/initializers/oj.rb).
  'oj_float' => floats.select(&:finite?).map { |f| [bits.(f), f.to_json] },
  'num' => num,
  'predicates' => operands.map { |v| [tag.(v), v.zero?, v.positive?, v.negative?, tag.(v.to_d), bits.(v.to_f)] },
  # BigDecimal#round(0) is an Integer; with places it stays a BigDecimal. Integer#round(places) is the Integer.
  'round' => (decimals + [0, 7]).product([0, 2, 4, 8]).map { |d, places| [tag.(d), places, tag.(d.round(places))] },
  'json' => [
    { a: BigDecimal('1.50'), b: 0, c: 1.0e-5, e: nil, f: true, g: [BigDecimal('0'), BigDecimal('1e-20'), BigDecimal('12345678901234567890.5')] },
    { "a<b&c>d e fé\"\\\n\r\t\u0001\u007f/\b\f" => "x\u{1F9A1}", 1 => { 2 => nil }, 'POR#12' => 'AAA.X' }
  ].map { |value| [value.as_json, value.to_json] },
  'times' => times.flat_map do |time|
    # London in winter is at no offset from UTC and is not UTC: Rails writes +00:00 there, and Z only for the zone UTC.
    [['UTC', 3], ['UTC', 9], ['UTC', 0], ['Warsaw', 3], ['Kathmandu', 6], ['Pacific Time (US & Canada)', 9], ['London', 3], ['Monrovia', 3]].map do |zone, digits|
      ActiveSupport::JSON::Encoding.time_precision = digits
      [nanos.(time), zone, digits, (zone == 'UTC' ? time : time.in_time_zone(zone)).to_json]
    end
  end,
  # Time - Time is a Float: whole seconds exactly, otherwise the nanoseconds divided by 1e9 as doubles.
  'to_d' => to_d,
  'time_minus' => times.product(times).map { |a, b| [nanos.(a), nanos.(b), bits.(a - b)] },
  'holding_keys' => holding_keys,
  'tax_lots' => tax_lots,
  'books' => books,
  'confirmed_exec_amounts' => confirmed,
  'split_factor' => split_factor,
  'timeframes' => timeframes,
  'pnl' => pnl,
  'grid_price' => grid_price,
  'thinned_marks' => thinned,
  'marked_at_market' => marked,
  'pnl_history' => merged,
  # Where a method's own arithmetic is ported from: record_figures_vectors.rb's header says when to record again.
  'account_transaction_adjustment' => AccountTransaction.entry_types.fetch('adjustment'),
  'ported_sources' => PORTED.to_h { |path| [path, Digest::SHA256.file(Rails.root.join(path)).hexdigest] },
  'versions' => { 'ruby' => RUBY_VERSION, 'bigdecimal' => BigDecimal::VERSION, 'oj' => Oj::VERSION, 'activesupport' => ActiveSupport.version.to_s }
}
ActiveSupport::JSON::Encoding.time_precision = 3

File.write(ARGV.fetch(0), "#{JSON.generate(vectors)}\n")
puts "wrote #{ARGV.fetch(0)}"
