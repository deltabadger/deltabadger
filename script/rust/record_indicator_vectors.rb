# Records Ticker::TechnicallyAnalyzable for rust/tests/technical.rs (engine::technical): RSI and the moving averages over
# seeded and degenerate close series, the high and low of a window with its candle timeframe, and the all-time high's
# incremental, deep and fallback reads with the `tickers` row they write. The candle read itself
# (Ticker#get_indicator_candles) is stubbed on the ticker: each call's start_at and timeframe are recorded and answered
# from the case's script, in order (7b-1's fixtures pin the venue reads). Values are recorded with their Ruby class.
#   bin/rails runner script/rust/record_indicator_vectors.rb rust/tests/fixtures/indicator_vectors.json
# Re-run (script/rust/repin) when a pinned source or gem changes.
require 'json'
require 'digest'
require 'active_support/testing/time_helpers'
extend ActiveSupport::Testing::TimeHelpers

PORTED = %w[app/models/ticker/technically_analyzable.rb app/models/utilities/time.rb app/models/ticker.rb app/models/exchange.rb
            app/models/exchanges/alpaca.rb app/models/bot/price_drop_limitable.rb script/rust/record_indicator_vectors.rb].freeze
# A fraction of a second down to the nanosecond, so the Float arithmetic of the ATH windows shows.
NOW = Time.utc(2026, 9, 2, 10, 0, Rational(1_123_456_789, 1_000_000_000))
TF = { '1h' => 1.hour, '4h' => 4.hours, '1d' => 1.day, '3d' => 3.days, '1w' => 1.week, '1M' => 1.month,
       '1m' => 1.minute, '5m' => 5.minutes, '15m' => 15.minutes, '30m' => 30.minutes }.freeze
TF_NAME = TF.to_h { |k, v| [v.inspect, k] }.freeze
WINDOWS = Bot::PriceDropLimitable::PRICE_DROP_LIMIT_TIME_WINDOW_CONDITIONS

def iso(t) = t.utc.strftime('%Y-%m-%dT%H:%M:%S.%NZ')

def enc(v)
  case v
  when nil then nil
  when Integer then { 'int' => v.to_s }
  when Float then { 'float' => [v].pack('G').unpack1('H*'), 's' => v.to_s }
  when BigDecimal then { 'dec' => v.to_s('F') }
  else { 'other' => "#{v.class}:#{v.inspect}" }
  end
end

# ---- The tickers: a Kraken crypto pair and an Alpaca stock (its history is restated).
kraken = Exchanges::Kraken.create!(name: 'Kraken', maker_fee: '0.25', taker_fee: '0.4')
alpaca = Exchanges::Alpaca.create!(name: 'Alpaca', maker_fee: '0.15', taker_fee: '0.25')
btc = Asset.create!(external_id: 'bitcoin', symbol: 'BTC', name: 'Bitcoin', category: 'Cryptocurrency')
eur = Asset.create!(external_id: 'EUR.FOREX', symbol: 'EUR', name: 'Euro', category: 'Currency')
usd = Asset.create!(external_id: 'usd', symbol: 'USD', name: 'US Dollar', category: 'Currency')
aapl = Asset.create!(external_id: 'AAPL.US', symbol: 'AAPL', name: 'Apple', category: 'Stock')
[[kraken, btc], [kraken, eur], [alpaca, aapl], [alpaca, usd]].each { |e, a| ExchangeAsset.create!(exchange: e, asset: a, available: true) }
TICKERS = {
  'kraken' => Ticker.create!(exchange: kraken, ticker: 'XBTEUR', base: 'XBT', quote: 'EUR', base_asset: btc, quote_asset: eur, base_decimals: 8,
                             quote_decimals: 5, price_decimals: 1, minimum_base_size: '0.00005', minimum_quote_size: '0.5'),
  'alpaca_stock' => Ticker.create!(exchange: alpaca, ticker: 'AAPL', base: 'AAPL', quote: 'USD', base_asset: aapl, quote_asset: usd, base_decimals: 9,
                                   quote_decimals: 2, price_decimals: 2, minimum_base_size: '0.000000001', minimum_quote_size: '1')
}.freeze

def row(ticker)
  r = Ticker.connection.select_one("SELECT typeof(ath) AS ath_type, CAST(ath AS TEXT) AS ath, ath_updated_at, updated_at FROM tickers WHERE id = #{ticker.id}")
  r.merge('ath_read' => enc(Ticker.find(ticker.id).ath))
end

def candle_rows(answer)
  answer.map { |at, *v| [Time.iso8601(at), *v.map { |x| BigDecimal(x) }] }
end

# Stub the candle read; returns the reads list it fills.
def script!(ticker, answers)
  reads = []
  queue = answers.dup
  ticker.define_singleton_method(:get_indicator_candles) do |start_at:, timeframe:|
    reads << { 'start_at' => iso(start_at), 'timeframe' => TF_NAME.fetch(timeframe.inspect) }
    answer = queue.shift or raise "unscripted candle read #{start_at} #{timeframe.inspect}"
    answer['failure'] ? Result::Failure.new(*answer['failure']) : Result::Success.new(candle_rows(answer['candles']))
  end
  reads
end

def result(r)
  r.success? ? { 'value' => enc(r.data) } : { 'failure' => r.errors }
end

# The period start that holds `t` for each timeframe, and `k` periods before a start.
def period_start(t, tf)
  case tf
  when '1w' then t.beginning_of_week
  when '1M' then t.beginning_of_month
  else Time.at((t.to_i / TF[tf].to_i) * TF[tf].to_i).utc
  end
end

def back(start, tf, k) = tf == '1M' ? start.advance(months: -k) : start - (k * TF[tf].to_i)

# ---- Close series. Each is a list of candles (o = h = l = c, volume 1) ending at the still-forming one, or (closed:
# a market that is shut) at the one before it, which is then kept.
rng = Random.new(2_026_102_6)
SERIES = []
add_series = lambda do |name, tf, closes, closed: false|
  last = period_start(NOW, tf)
  last = back(last, tf, 1) if closed
  times = (0...closes.size).map { |i| back(last, tf, closes.size - 1 - i) }
  SERIES << { 'name' => name, 'timeframe' => tf, 'candles' => times.zip(closes).map { |t, c| [iso(t), c, c, c, c, '1'] } }
end
fmt = ->(x, places) { BigDecimal(x.to_s).round(places).to_d.to_s('F') }
160.times do |i|
  tf = %w[1d 1h 4h 3d 1w 1M].fetch(i % 6)
  scale = [0.00000123, 0.4, 31.0, 2512.0, 64_123.0, 1_234_567.0].fetch(rng.rand(6))
  places = [0, 1, 2, 4, 6, 8, 10].fetch(rng.rand(7))
  len = [rng.rand(1..40), rng.rand(10..160), rng.rand(100..280)].fetch(rng.rand(3))
  x = scale
  closes = Array.new(len) do
    step = rng.rand(10).zero? ? 0 : (rng.rand - 0.5) * scale * 0.06
    x = [x + step, scale / 50].max
    fmt.(x, places)
  end
  add_series.("seeded_#{i}", tf, closes, closed: (i % 7).zero?)
end
# Degenerate series (D-COND-RSI-DEGENERATE): Rails' values recorded as they are.
add_series.('flat_30', '1d', ['64000.5'] * 31)
add_series.('all_gain_30', '1d', (0..30).map { |k| (64_000 + k).to_s })
add_series.('all_loss_30', '1d', (0..30).map { |k| (64_000 - k).to_s })
add_series.('gain_15', '1d', (0..15).map { |k| (64_000 + k).to_s })
add_series.('closes_14', '1d', (0..14).map { |k| (64_000 + k).to_s })
add_series.('one_loss_then_gains', '1d', ['64010'] + (0..40).map { |k| (64_000 + k).to_s })
add_series.('one_gain_then_flat', '1d', ['64000', '64001'] + (['64001'] * 30))
add_series.('flat_then_one_loss', '1d', (['64000'] * 30) + ['63999', '63999'])
add_series.('single_close', '1d', ['64000.5', '64000.5'])
add_series.('only_forming', '1d', ['64000.5'])
add_series.('ma_9_closes', '1d', (['64050.05'] * 9) + ['65050.05'])
add_series.('ma_8_closes', '1d', (['64050.05'] * 8) + ['65050.05'])
add_series.('many_digits', '1d', (0..40).map { |k| "#{64_000 + (k * 7 % 13)}.#{'123456789' * 3}#{k}" })
add_series.('empty', '1d', [])
add_series.('closed_market', '1d', (0..30).map { |k| (100 + (k * 3 % 7)).to_s }, closed: true)

ticker = TICKERS.fetch('kraken')
indicators = []
SERIES.each_with_index do |s, si|
  [1, 9, 12, 14, 26].each do |period|
    %w[rsi sma ema].each do |kind|
      next if si < 160 && (si % 3) != %w[rsi sma ema].index(kind) && period != 14 # keep the grid within the plan's size
      pd = [0, 1, 2, 5, 8].fetch((si + period) % 5)
      ticker.price_decimals = pd
      Rails.cache = ActiveSupport::Cache::MemoryStore.new
      reads = script!(ticker, [{ 'candles' => s['candles'] }])
      tf = TF.fetch(s['timeframe'])
      r = travel_to(NOW, with_usec: true) do
        kind == 'rsi' ? ticker.get_rsi_value(timeframe: tf, period:) : ticker.get_moving_average_value(timeframe: tf, period:, type: kind)
      end
      indicators << { 'series' => si, 'kind' => kind, 'period' => period, 'price_decimals' => pd, 'reads' => reads }.merge(result(r))
    end
  end
end
ticker.price_decimals = 1

# ---- Sequences: calls in order on one cache, each at its own time, with the row after it.
sequences = []
sequence = lambda do |name, ticker_key, before, steps|
  t = TICKERS.fetch(ticker_key)
  t.update_columns(ath: before['ath'], ath_updated_at: before['ath_updated_at'] && Time.iso8601(before['ath_updated_at']),
                   updated_at: Time.utc(2026, 1, 1))
  t.reload
  Rails.cache = ActiveSupport::Cache::MemoryStore.new
  recorded = steps.map do |step|
    t.reload
    reads = script!(t, step['answers'])
    at = Time.iso8601(step['at'])
    r = begin
      travel_to(at, with_usec: true) do
      case step['call']
      when 'high' then t.get_high_of_last(duration: WINDOWS.fetch(step['window']))
      when 'low' then t.get_low_of_last(duration: WINDOWS.fetch(step['window']))
      when 'rsi' then t.get_rsi_value(timeframe: TF.fetch(step['timeframe']), period: step['period'])
      else t.get_moving_average_value(timeframe: TF.fetch(step['timeframe']), period: step['period'], type: step['call'])
      end
      end
    rescue StandardError => e
      e
    end
    step.merge('reads' => reads, 'row' => row(t)).merge(r.is_a?(Exception) ? { 'raised' => "#{r.class}: #{r.message}" } : result(r))
  end
  sequences << { 'name' => name, 'ticker' => ticker_key, 'before' => before, 'steps' => recorded }
end

at = ->(offset) { iso(NOW + offset) }
c = ->(t, h, l = h) { [iso(t), h, h, l, l, '1'] }
ok = ->(*rows) { { 'candles' => rows } }
EMPTY = { 'candles' => [] }.freeze
FAIL = { 'failure' => ['Kraken: EService:Unavailable'] }.freeze
unseeded = { 'ath' => nil, 'ath_updated_at' => nil }
seeded = ->(ath, age) { { 'ath' => ath, 'ath_updated_at' => iso(Time.utc(2026, 9, 2, 10, 0, Rational(1_123_456, 1_000_000)) - age) } }
h = ->(n) { NOW - n.hours }

# High and low of a finite window: each window's timeframe, the forming candle included, empty is nil, a failure as it is.
WINDOWS.each_key do |w|
  next if w == 'ath'

  sequence.("high_#{w}", 'kraken', unseeded, [{ 'at' => at.(0), 'call' => 'high', 'window' => w, 'answers' => [ok.(c.(h.(5), '64100.5'), c.(h.(1), '64200.25'), c.(NOW, '64150'))] }])
  sequence.("low_#{w}", 'kraken', unseeded, [{ 'at' => at.(0), 'call' => 'low', 'window' => w, 'answers' => [ok.(c.(h.(5), '1', '63900.5'), c.(h.(1), '1', '63800.125'), c.(NOW, '1', '63850'))] }])
  sequence.("low_#{w}_empty", 'kraken', unseeded, [{ 'at' => at.(0), 'call' => 'low', 'window' => w, 'answers' => [EMPTY] }])
  sequence.("high_#{w}_failure", 'kraken', unseeded, [{ 'at' => at.(0), 'call' => 'high', 'window' => w, 'answers' => [FAIL] }])
end
# The 20 s cache: kept at +19.9 s, read again at +20 s; a failure is never kept.
sequence.('low_cache_20s', 'kraken', unseeded, [
  { 'at' => at.(0), 'call' => 'low', 'window' => 'twenty_four_hours', 'answers' => [ok.(c.(h.(1), '1', '63800'))] },
  { 'at' => at.(19.9), 'call' => 'low', 'window' => 'twenty_four_hours', 'answers' => [] },
  { 'at' => at.(20), 'call' => 'low', 'window' => 'twenty_four_hours', 'answers' => [ok.(c.(h.(1), '1', '63700'))] },
  { 'at' => at.(21), 'call' => 'low', 'window' => 'seven_days', 'answers' => [FAIL] },
  { 'at' => at.(22), 'call' => 'low', 'window' => 'seven_days', 'answers' => [ok.(c.(h.(1), '1', '63600'))] }
])

# ATH: the deep walk (20 years of days) seeds it; then incremental from ath_updated_at, its timeframe by the age.
sequence.('ath_deep_seed', 'kraken', unseeded, [
  { 'at' => at.(0), 'call' => 'high', 'window' => 'ath', 'answers' => [ok.(c.(Time.utc(2021, 11, 10), '59000.5'), c.(Time.utc(2025, 10, 6), '107000.25'), c.(Time.utc(2026, 9, 2), '64000'))] },
  { 'at' => at.(10), 'call' => 'high', 'window' => 'ath', 'answers' => [] },
  { 'at' => at.(25), 'call' => 'high', 'window' => 'ath', 'answers' => [ok.(c.(NOW, '64000'))] },
  { 'at' => at.(3600), 'call' => 'high', 'window' => 'ath', 'answers' => [ok.(c.(NOW + 3000, '108000.5'))] }
])
[3, 11.9, 12, 59.9, 60, 179, 180, 359, 360, 719, 720, 2000].each do |n|
  sequence.("ath_incremental_#{n}h", 'kraken', seeded.('70000.5', n.hours), [
    { 'at' => at.(0), 'call' => 'high', 'window' => 'ath', 'answers' => [ok.(c.(h.(1), '70000.75'))] }
  ])
end
sequence.('ath_incremental_lower_keeps', 'kraken', seeded.('70000.5', 5.hours), [{ 'at' => at.(0), 'call' => 'high', 'window' => 'ath', 'answers' => [ok.(c.(h.(1), '69000'))] }])
sequence.('ath_incremental_empty_keeps', 'kraken', seeded.('70000.5', 5.hours), [{ 'at' => at.(0), 'call' => 'high', 'window' => 'ath', 'answers' => [EMPTY] }])
sequence.('ath_incremental_failure', 'kraken', seeded.('70000.5', 5.hours), [{ 'at' => at.(0), 'call' => 'high', 'window' => 'ath', 'answers' => [FAIL] }])
sequence.('ath_incremental_float_digits', 'kraken', seeded.('0.30000000000000004', 5.hours), [{ 'at' => at.(0), 'call' => 'high', 'window' => 'ath', 'answers' => [ok.(c.(h.(1), '0.1'))] }])
sequence.('ath_incremental_many_digits', 'kraken', seeded.('70000.5', 5.hours), [{ 'at' => at.(0), 'call' => 'high', 'window' => 'ath', 'answers' => [ok.(c.(h.(1), '70000.523456789012345678'))] }])
# The fallback windows, shortest last, after an empty deep walk; the first non-empty one seeds.
(0..5).each do |found|
  answers = [EMPTY] + ([EMPTY] * found) + [ok.(c.(h.(30), '65000.5'))]
  sequence.("ath_fallback_#{found}", 'kraken', unseeded, [{ 'at' => at.(0), 'call' => 'high', 'window' => 'ath', 'answers' => answers }])
end
sequence.('ath_fallback_failure', 'kraken', unseeded, [{ 'at' => at.(0), 'call' => 'high', 'window' => 'ath', 'answers' => [EMPTY, EMPTY, FAIL] }])
sequence.('ath_deep_failure', 'kraken', unseeded, [{ 'at' => at.(0), 'call' => 'high', 'window' => 'ath', 'answers' => [FAIL] }])
# A known-empty seed: every window empty marks it for an hour (the deep walk is skipped; the nil that answers is itself
# kept 20 s), then it walks again.
sequence.('ath_seed_empty', 'kraken', unseeded, [
  { 'at' => at.(0), 'call' => 'high', 'window' => 'ath', 'answers' => [EMPTY] * 7 },
  { 'at' => at.(30), 'call' => 'high', 'window' => 'ath', 'answers' => [] },
  { 'at' => at.(3599), 'call' => 'high', 'window' => 'ath', 'answers' => [] },
  { 'at' => at.(3601), 'call' => 'high', 'window' => 'ath', 'answers' => [EMPTY, ok.(c.(h.(30), '65000.5'))] },
  { 'at' => at.(3625), 'call' => 'high', 'window' => 'ath', 'answers' => [EMPTY, ok.(c.(h.(30), '65000.5'))] }
])
# ath without ath_updated_at walks deep, never lowers it, and is never marked empty.
sequence.('ath_without_updated_at_lower', 'kraken', { 'ath' => '70000.5', 'ath_updated_at' => nil }, [{ 'at' => at.(0), 'call' => 'high', 'window' => 'ath', 'answers' => [ok.(c.(h.(30), '65000'))] }])
sequence.('ath_without_updated_at_fallback', 'kraken', { 'ath' => '70000.5', 'ath_updated_at' => nil }, [{ 'at' => at.(0), 'call' => 'high', 'window' => 'ath', 'answers' => [EMPTY, ok.(c.(h.(30), '75000'))] }])
sequence.('ath_without_updated_at_all_empty', 'kraken', { 'ath' => '70000.5', 'ath_updated_at' => nil }, [
  { 'at' => at.(0), 'call' => 'high', 'window' => 'ath', 'answers' => [EMPTY] * 7 },
  { 'at' => at.(30), 'call' => 'high', 'window' => 'ath', 'answers' => [EMPTY] * 7 }
])
# A restated history (an Alpaca stock): the full walk replaces the high, a fallback window only raises it.
sequence.('ath_restated_replaces', 'alpaca_stock', seeded.('500.25', 5.hours), [{ 'at' => at.(0), 'call' => 'high', 'window' => 'ath', 'answers' => [ok.(c.(h.(3000), '120.5'), c.(h.(30), '118'))] }])
sequence.('ath_restated_fallback_keeps', 'alpaca_stock', seeded.('500.25', 5.hours), [{ 'at' => at.(0), 'call' => 'high', 'window' => 'ath', 'answers' => [EMPTY, ok.(c.(h.(30), '118'))] }])
sequence.('ath_restated_all_empty', 'alpaca_stock', seeded.('500.25', 5.hours), [{ 'at' => at.(0), 'call' => 'high', 'window' => 'ath', 'answers' => [EMPTY] * 7 }])
sequence.('ath_restated_unseeded', 'alpaca_stock', unseeded, [{ 'at' => at.(0), 'call' => 'high', 'window' => 'ath', 'answers' => [ok.(c.(h.(30), '118.5'))] }])
# The leap day: 2028-02-29 less five years, then a day.
sequence.('ath_fallback_leap_day', 'kraken', unseeded, [{ 'at' => '2028-02-29T10:00:01.123456789Z', 'call' => 'high', 'window' => 'ath',
                                                         'answers' => [EMPTY, ok.(c.(Time.utc(2028, 2, 1), '65000.5'))] }])

# RSI and the moving averages are kept until the candle closes: the same day reuses them, the next day reads again; a
# failure (too few closes, no candles) is never kept. Monthly and weekly candles close at the month's and week's end.
flat = SERIES.find { |s| s['name'] == 'seeded_0' }
gain = SERIES.find { |s| s['name'] == 'gain_15' }
sequence.('rsi_cache_until_close', 'kraken', unseeded, [
  { 'at' => at.(0), 'call' => 'rsi', 'timeframe' => '1d', 'period' => 14, 'answers' => [{ 'candles' => gain['candles'] }] },
  { 'at' => '2026-09-02T23:59:59.999Z', 'call' => 'rsi', 'timeframe' => '1d', 'period' => 14, 'answers' => [] },
  { 'at' => '2026-09-03T00:00:00.000Z', 'call' => 'rsi', 'timeframe' => '1d', 'period' => 14, 'answers' => [{ 'candles' => gain['candles'].first(14) }] },
  { 'at' => '2026-09-03T00:00:01.000Z', 'call' => 'rsi', 'timeframe' => '1d', 'period' => 14, 'answers' => [EMPTY] },
  { 'at' => '2026-09-03T00:00:02.000Z', 'call' => 'rsi', 'timeframe' => '1d', 'period' => 14, 'answers' => [FAIL] }
])
sequence.('ma_cache_shared_by_type', 'kraken', unseeded, [
  { 'at' => at.(0), 'call' => 'sma', 'timeframe' => '1M', 'period' => 9, 'answers' => [{ 'candles' => flat['candles'] }] },
  { 'at' => '2026-09-30T23:59:59.999Z', 'call' => 'ema', 'timeframe' => '1M', 'period' => 9, 'answers' => [] },
  { 'at' => '2026-10-01T00:00:00.000Z', 'call' => 'ema', 'timeframe' => '1M', 'period' => 9, 'answers' => [{ 'candles' => flat['candles'] }] },
  { 'at' => '2026-10-01T00:00:01.000Z', 'call' => 'sma', 'timeframe' => '1w', 'period' => 9, 'answers' => [{ 'candles' => flat['candles'] }] },
  { 'at' => '2026-10-04T23:59:59.999Z', 'call' => 'sma', 'timeframe' => '1w', 'period' => 9, 'answers' => [] },
  { 'at' => '2026-10-05T00:00:00.000Z', 'call' => 'sma', 'timeframe' => '1w', 'period' => 9, 'answers' => [{ 'candles' => flat['candles'].first(3) }] }
])

out = {
  'sources' => PORTED.to_h { |f| [f, Digest::SHA256.file(Rails.root.join(f)).hexdigest] },
  'gems' => { 'ruby-technical-analysis' => Gem.loaded_specs['ruby-technical-analysis'].version.to_s, 'bigdecimal' => BigDecimal::VERSION },
  'now' => iso(NOW),
  'series' => SERIES,
  'indicators' => indicators,
  'sequences' => sequences
}
File.write(ARGV.fetch(0), "#{JSON.pretty_generate(out)}\n")
puts "#{indicators.size} indicator cases, #{sequences.size} sequences"
