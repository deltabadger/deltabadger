# Records Exchange#get_indicator_candles (the candle read the conditions use) for rust/tests/candles.rs: the candles
# Rails returns (time ISO8601, then open, high, low, close and volume as to_s('F')), its Failure's errors, or what it
# raised, and every request it made, in order. Every case runs at NOW (travel_to): Binance's walk ends against
# `timeframe.ago`.
#   bin/rails runner script/rust/record_candles.rb kraken rust/tests/fixtures/kraken_candles.json
#   (binance, binance_us, alpaca, kucoin, bitget, bybit, mexc, bitrue, bitvavo, gemini, hyperliquid, coinbase and bingx likewise)
# Each case's bodies are served at the Faraday adapter beneath honeymaker, so its middleware, #with_rescue and the
# exchange's own parsing and CandleBuilder all run. Nothing leaves the machine. Re-run (script/rust/repin) when a file
# under "sources" changes. One venue per fixture; later venues add a section below.
require 'json'
require 'digest'
require 'active_support/testing/time_helpers'

class Unscripted < Exception; end # rubocop:disable Lint/InheritException

# rubocop:disable Style/OneClassPerFile

Net::HTTP.prepend(Module.new { def connect = raise(Unscripted, "real connection to #{address}") })

module ScriptedCandles
  mattr_accessor :http, :requests

  NETWORK = {
    'pre_send' => -> { Faraday::ConnectionFailed.new(Errno::ECONNREFUSED.new('connect(2)')) },
    'post_send' => -> { Faraday::TimeoutError.new(Net::ReadTimeout.new) }
  }.freeze

  module Adapter
    def call(env)
      raise Unscripted, "no case is running: #{env.url}" if ScriptedCandles.http.nil?

      query = Rack::Utils.parse_query(env.url.query).map { |k, v| "#{k}=#{v}" }.sort.join('&')
      key = "#{env.method.to_s.upcase} #{env.url.path}#{"?#{query}" unless query.empty?}"
      # A POST's body is recorded with it (Hyperliquid's one read); it is answered by its method and path.
      ScriptedCandles.requests << (env.body.present? ? "#{key} #{env.body}" : key)
      queue = ScriptedCandles.http[key] or raise Unscripted, "unscripted call #{key}"
      reply = queue.size > 1 ? queue.shift : queue.first
      raise NETWORK.fetch(reply['network']).call if reply['network']

      body = reply['body'].is_a?(String) ? reply['body'] : JSON.generate(reply['body'])
      env.response = Faraday::Response.new
      save_response(env, reply.fetch('status', 200), body, { 'Content-Type' => 'application/json' })
      @app.call(env)
    end
  end
end
# rubocop:enable Style/OneClassPerFile
Faraday::Adapter.lookup_middleware(:net_http_persistent).prepend(ScriptedCandles::Adapter)

def ok(body) = [{ 'status' => 200, 'body' => body }]
def status(code, body) = [{ 'status' => code, 'body' => body }]
def network(kind) = [{ 'network' => kind, 'message' => kind }]

TIMEFRAMES = { '1m' => 1.minute, '5m' => 5.minutes, '15m' => 15.minutes, '30m' => 30.minutes, '1h' => 1.hour, '4h' => 4.hours,
               '1d' => 1.day, '3d' => 3.days, '1w' => 1.week, '1M' => 1.month }.freeze
TickerStub = Struct.new(:ticker, :base, :base_asset)
AssetStub = Struct.new(:category)
NOW = Time.utc(2026, 9, 2, 10, 0, 1)

# ---- Kraken: one OHLC read since a second before the start; rows from that second on; 3d/1w/1M built from daily.
KR_PAIR = 'XXBTZEUR'.freeze
KR_MINUTES = { '1m' => 1, '5m' => 5, '15m' => 15, '30m' => 30, '1h' => 60, '4h' => 240 }.freeze
def kr_interval(tf) = KR_MINUTES.fetch(tf, 1440)
def kr_key(tf, start_at) = "GET /0/public/OHLC?interval=#{kr_interval(tf)}&pair=#{KR_PAIR}&since=#{(start_at - 1.second).to_i}"
# [time, open, high, low, close, vwap, volume, count], prices as Kraken sends them: strings.
def kr_row(at, i)
  [at, "#{64_000 + (i * 37 % 101)}.5", "#{64_200 + (i * 53 % 97)}.25", "#{63_800 - (i * 29 % 89)}.125", "#{64_000 + (i * 71 % 103)}.75",
   '64001.1', "#{(i % 5) + 1}.0001", 10 + i]
end
def kr_body(rows, last: 1_788_000_000) = { 'error' => [], 'result' => { KR_PAIR => rows, 'last' => last } }
def kr_rows(from, step, count) = (0...count).map { |i| kr_row(from + (i * step), i) }

kraken = []
# The start: 2026-09-02 10:00:00.5 UTC (a second before it is 09:59:59.5, so `since` is 09:59:59).
start = Time.utc(2026, 9, 2, 10, 0, 0.5r)
TIMEFRAMES.each_key do |tf|
  step = kr_interval(tf) * 60
  aligned = (start.to_i / step) * step
  # The venue answers from the candle holding `since`: the one before the start is dropped.
  first = aligned - (step * 2)
  kraken << ["#{tf}_normal", tf, start, { kr_key(tf, start) => ok(kr_body(kr_rows(first, step, 40))) }]
  rows = kr_rows(first, step, 40)
  rows.delete_at(10)
  rows.delete_at(20)
  kraken << ["#{tf}_gap", tf, start, { kr_key(tf, start) => ok(kr_body(rows)) }]
  # The candle whose period holds the start: kept only when it opens at or after a second before the start.
  kraken << ["#{tf}_holding_the_start", tf, start, { kr_key(tf, start) => ok(kr_body(kr_rows(aligned, step, 1))) }]
  kraken << ["#{tf}_empty", tf, start, { kr_key(tf, start) => ok(kr_body([])) }]
  kraken << ["#{tf}_http_500", tf, start, { kr_key(tf, start) => status(500, 'upstream failed') }]
end

# CandleBuilder over daily candles: calendar weeks (Monday), calendar months, epoch-aligned 3 days; a group starting
# before the first candle is dropped.
day = 86_400
[['3d', Time.utc(2026, 7, 1)], ['3d', Time.utc(2026, 7, 2)], ['1w', Time.utc(2026, 7, 1)], ['1w', Time.utc(2026, 7, 6)],
 ['1M', Time.utc(2026, 6, 15)], ['1M', Time.utc(2026, 6, 1)], ['1M', Time.utc(2026, 1, 20)]].each do |tf, from|
  name = "#{tf}_built_from_#{from.strftime('%Y%m%d')}"
  kraken << [name, tf, from, { kr_key(tf, from) => ok(kr_body(kr_rows(from.to_i, day, 75))) }]
end
kraken << ['1w_built_single_day', '1w', Time.utc(2026, 7, 6), { kr_key('1w', Time.utc(2026, 7, 6)) => ok(kr_body(kr_rows(Time.utc(2026, 7, 6).to_i, day, 1))) }]
kraken << ['1M_built_leap_february', '1M', Time.utc(2028, 1, 30), { kr_key('1M', Time.utc(2028, 1, 30)) => ok(kr_body(kr_rows(Time.utc(2028, 1, 30).to_i, day, 45))) }]
# Built groups follow the rows as sent: an out-of-order day joins its own group.
kraken << ['1w_built_unsorted', '1w', Time.utc(2026, 7, 6),
           { kr_key('1w', Time.utc(2026, 7, 6)) => ok(kr_body(kr_rows(Time.utc(2026, 7, 6).to_i, day, 20).values_at(0, 1, 15, 2, 3, 8, 9, 16, 4))) }]

# The start's own second: the filter is against the start less a second, fractions kept.
exact = Time.utc(2026, 9, 2, 10, 0, 0)
kraken << ['row_a_second_before_an_exact_start_kept', '1m', exact,
           { kr_key('1m', exact) => ok(kr_body([kr_row(exact.to_i - 2, 0), kr_row(exact.to_i - 1, 1), kr_row(exact.to_i, 2)])) }]
kraken << ['row_a_second_before_a_fractional_start_dropped', '1m', start,
           { kr_key('1m', start) => ok(kr_body([kr_row(start.to_i - 1, 0), kr_row(start.to_i, 1), kr_row(start.to_i + 1, 2)])) }]
far = Time.utc(2026, 9, 2, 10, 0, 0.999999r)
kraken << ['row_at_the_start_second_of_a_late_fraction_dropped', '1m', far,
           { kr_key('1m', far) => ok(kr_body([kr_row(far.to_i - 1, 0), kr_row(far.to_i, 1)])) }]
# Kraken's 720: whatever `since` asks, the newest 720 come back, and Rails keeps every one from the start on.
deep = Time.utc(2026, 8, 1)
kraken << ['720_cap_kept_as_sent', '1h', deep, { kr_key('1h', deep) => ok(kr_body(kr_rows(((start.to_i / 3600) - 719) * 3600, 3600, 720))) }]
kraken << ['720_cap_daily_from_years_back', '1d', Time.utc(2006, 9, 1),
           { kr_key('1d', Time.utc(2006, 9, 1)) => ok(kr_body(kr_rows(((start.to_i / day) - 719) * day, day, 720))) }]

# Failures and unreadable answers.
k = ->(tf = '1h') { kr_key(tf, start) }
kraken << ['error_unknown_pair', '1h', start, { k.call => ok({ 'error' => ['EQuery:Unknown asset pair'] }) }]
kraken << ['error_two', '1h', start, { k.call => ok({ 'error' => ['EService:Unavailable', 'EGeneral:Internal error'] }) }]
kraken << ['error_with_result', '1d', start, { k.call('1d') => ok({ 'error' => ['EGeneral:Too many requests'], 'result' => { KR_PAIR => [] } }) }]
kraken << ['error_missing', '1h', start, { k.call => ok({ 'result' => { KR_PAIR => [] } }) }]
kraken << ['error_null', '1h', start, { k.call => ok({ 'error' => nil, 'result' => { KR_PAIR => [] } }) }]
kraken << ['result_missing', '1h', start, { k.call => ok({ 'error' => [] }) }]
kraken << ['result_null', '1h', start, { k.call => ok({ 'error' => [], 'result' => nil }) }]
kraken << ['result_last_only', '1h', start, { k.call => ok({ 'error' => [], 'result' => { 'last' => 1 } }) }]
kraken << ['result_empty', '1h', start, { k.call => ok({ 'error' => [], 'result' => {} }) }]
kraken << ['result_last_first', '1h', start,
           { k.call => ok({ 'error' => [], 'result' => { 'last' => 1, KR_PAIR => kr_rows((start.to_i / 3600) * 3600, 3600, 2) } }) }]
kraken << ['result_rows_null', '1h', start, { k.call => ok({ 'error' => [], 'result' => { KR_PAIR => nil, 'last' => 1 } }) }]
kraken << ['http_500_empty', '1h', start, { k.call => status(500, '') }]
kraken << ['http_502', '1h', start, { k.call => status(502, '<html>bad gateway</html>') }]
kraken << ['http_400_json', '1h', start, { k.call => status(400, { 'error' => ['EGeneral:Invalid arguments'] }) }]
kraken << ['http_401', '1h', start, { k.call => status(401, 'unauthorized') }]
kraken << ['http_429', '1h', start, { k.call => status(429, 'slow down') }]
kraken << ['not_json', '1h', start, { k.call => ok('<html>') }]
kraken << ['json_array', '1h', start, { k.call => ok([1, 2]) }]
kraken << ['transport_pre_send', '1h', start, { k.call => network('pre_send') }]
kraken << ['transport_timeout', '1h', start, { k.call => network('post_send') }]

# Numbers as Rails reads them (`to_d`).
now_h = (start.to_i / 3600) * 3600
row = ->(**over) { r = kr_row(now_h, 0); over.each { |i, v| r[i.to_s.delete('f').to_i] = v }; r }
{ 'numbers_as_json_numbers' => [now_h, 64_000.5, 64_100, 63_900.25, 64_050, 1, 2.5, 3],
  'float_digits' => [now_h, 1.2345678901234567, '2', '1', '1.5', '1', 1e-07, 1],
  'many_digits' => [now_h, '64000.123456789012345678901234567890', '64000.2', '63999.9', '64000.0', '1', '0.00000001', 1],
  'exponent_strings' => [now_h, '6.4e4', '6.41E4', '6.39e4', '6.4e4', '1', '1e-3', 1],
  'open_null' => row.call(f1: nil),
  'open_word' => row.call(f1: 'x'),
  'close_blank' => row.call(f4: ''),
  'volume_null' => row.call(f6: nil),
  'short_row' => [now_h, '1', '2', '0.5', '1.5'],
  'time_string' => row.call(f0: now_h.to_s),
  'time_float' => row.call(f0: now_h + 0.5),
  'time_null' => row.call(f0: nil) }.each do |name, r|
  kraken << ["row_#{name}", '1h', start, { k.call => ok(kr_body([r])) }]
end
kraken << ['row_not_an_array', '1h', start, { k.call => ok(kr_body(['x'])) }]

# ---- Binance (Binance.US runs the same #get_candles at its own host): pages of up to 1000 klines from startTime (ms,
# the start's whole seconds); the next page starts a second after the last candle; the walk ends at an empty page or at
# a last candle that opened after `timeframe.ago` (a calendar month for 1M). Nothing is filtered or built.
BN_SYMBOL = 'BTCUSDT'.freeze
def bn_key(tf, start_s) = "GET /api/v3/klines?interval=#{tf}&limit=1000&startTime=#{start_s * 1000}&symbol=#{BN_SYMBOL}&timeZone=0"
# [open time, open, high, low, close, volume, close time, quote volume, trades, taker base, taker quote, ignore]
def bn_row(ms, i)
  [ms, "#{64_000 + (i * 37 % 101)}.50000000", "#{64_200 + (i * 53 % 97)}.25000000", "#{63_800 - (i * 29 % 89)}.12500000",
   "#{64_000 + (i * 71 % 103)}.75000000", "#{(i % 5) + 1}.00010000", ms + 59_999, '64001.10000000', 10 + i, '0.5', '32000.1', '0']
end
def bn_rows(times) = times.each_with_index.map { |at, i| bn_row(at * 1000, i) }

binance = []
bn_start = start.to_i
minute = bn_start - (bn_start % 60)
TIMEFRAMES.each do |tf, duration|
  step = duration.to_i
  # Forty candles, the last the one forming at the start: past `timeframe.ago`, so one page.
  times = (0...40).map { |i| minute - ((39 - i) * step) }
  binance << ["#{tf}_normal", tf, start, { bn_key(tf, bn_start) => ok(bn_rows(times)) }]
  binance << ["#{tf}_gap", tf, start, { bn_key(tf, bn_start) => ok(bn_rows(times.values_at(0..9, 11..20, 22..39))) }]
  binance << ["#{tf}_empty", tf, start, { bn_key(tf, bn_start) => ok([]) }]
  binance << ["#{tf}_http_500", tf, start, { bn_key(tf, bn_start) => status(500, 'upstream failed') }]
end

# Paging: a page whose last candle opened at or before `timeframe.ago` asks again from a second after it.
deep = Time.utc(2026, 8, 1)
h = ->(*hours) { hours.map { |n| deep.to_i + (n * 3600) } }
binance << ['1h_two_pages', '1h', deep, { bn_key('1h', deep.to_i) => ok(bn_rows(h.call(0, 1, 2))),
                                          bn_key('1h', h.call(2)[0] + 1) => ok(bn_rows(h.call(3, 4) + [minute])) }]
binance << ['1h_three_pages', '1h', deep, { bn_key('1h', deep.to_i) => ok(bn_rows(h.call(0, 1))),
                                            bn_key('1h', h.call(1)[0] + 1) => ok(bn_rows(h.call(2, 3))),
                                            bn_key('1h', h.call(3)[0] + 1) => ok(bn_rows([minute])) }]
binance << ['1h_page_then_empty', '1h', deep, { bn_key('1h', deep.to_i) => ok(bn_rows(h.call(0, 1))),
                                                bn_key('1h', h.call(1)[0] + 1) => ok([]) }]
binance << ['1h_page_then_500', '1h', deep, { bn_key('1h', deep.to_i) => ok(bn_rows(h.call(0, 1))),
                                              bn_key('1h', h.call(1)[0] + 1) => status(500, '') }]
binance << ['1h_page_then_error', '1h', deep, { bn_key('1h', deep.to_i) => ok(bn_rows(h.call(0, 1))),
                                                bn_key('1h', h.call(1)[0] + 1) => status(400, { 'code' => -1121, 'msg' => 'Invalid symbol.' }) }]
# A page out of order: the next start follows the LAST row, not the latest.
binance << ['1h_page_unsorted', '1h', deep, { bn_key('1h', deep.to_i) => ok(bn_rows(h.call(5, 0))),
                                              bn_key('1h', h.call(0)[0] + 1) => ok(bn_rows([minute])) }]
# A start with a fraction asks from its whole second.
binance << ['1h_fractional_start', '1h', Time.utc(2026, 8, 1, 0, 0, 0.75r), { bn_key('1h', deep.to_i) => ok(bn_rows([minute])) }]
# The cutoff in milliseconds: a last candle opening exactly at `1.day.ago` (whole seconds, times 1000) pages on; a
# millisecond later ends the walk. Its time is read in whole seconds.
cut = (NOW - 1.day).to_i
binance << ['1d_last_at_the_cutoff_pages_on', '1d', deep, { bn_key('1d', deep.to_i) => ok([bn_row(cut * 1000, 0)]),
                                                           bn_key('1d', cut + 1) => ok([]) }]
binance << ['1d_last_a_millisecond_past_the_cutoff_ends', '1d', deep, { bn_key('1d', deep.to_i) => ok([bn_row((cut * 1000) + 1, 0)]) }]
binance << ['1d_millisecond_times', '1d', deep, { bn_key('1d', deep.to_i) => ok([bn_row((minute * 1000) + 999, 0)]) }]
# 1.month.ago is a calendar month (2026-08-02T10:00:01Z), not 30.44 days.
binance << ['1M_cutoff_is_a_calendar_month_ends', '1M', deep, { bn_key('1M', deep.to_i) => ok(bn_rows([Time.utc(2026, 8, 2, 12).to_i])) }]
binance << ['1M_cutoff_is_a_calendar_month_pages_on', '1M', deep,
            { bn_key('1M', deep.to_i) => ok(bn_rows([Time.utc(2026, 8, 2, 10).to_i])), bn_key('1M', Time.utc(2026, 8, 2, 10).to_i + 1) => ok([]) }]
binance << ['1w_cutoff_seven_days', '1w', deep, { bn_key('1w', deep.to_i) => ok(bn_rows([(NOW - 7.days).to_i])),
                                                  bn_key('1w', (NOW - 7.days).to_i + 1) => ok([]) }]

# Failures and unreadable answers.
b = ->(tf = '1h') { bn_key(tf, bn_start) }
binance << ['error_invalid_symbol', '1h', start, { b.call => status(400, { 'code' => -1121, 'msg' => 'Invalid symbol.' }) }]
binance << ['error_blank_msg', '1h', start, { b.call => status(400, { 'code' => -1, 'msg' => '' }) }]
binance << ['error_no_msg', '1h', start, { b.call => status(400, { 'code' => -1 }) }]
binance << ['error_invalid_interval', '1h', start, { b.call => status(400, { 'code' => -1120, 'msg' => 'Invalid interval.' }) }]
binance << ['http_500_empty', '1h', start, { b.call => status(500, '') }]
binance << ['http_502', '1h', start, { b.call => status(502, '<html>bad gateway</html>') }]
binance << ['http_401', '1h', start, { b.call => status(401, { 'code' => -2015, 'msg' => 'Invalid API-key, IP, or permissions for action.' }) }]
binance << ['http_403', '1h', start, { b.call => status(403, '') }]
binance << ['http_418', '1h', start, { b.call => status(418, { 'code' => -1003, 'msg' => 'Way too many requests; IP banned.' }) }]
binance << ['http_429', '1h', start, { b.call => status(429, { 'code' => -1003, 'msg' => 'Too many requests.' }) }]
binance << ['not_json', '1h', start, { b.call => ok('<html>') }]
binance << ['json_object', '1h', start, { b.call => ok({ 'code' => 0, 'msg' => 'ok' }) }]
binance << ['transport_pre_send', '1h', start, { b.call => network('pre_send') }]
binance << ['transport_timeout', '1h', start, { b.call => network('post_send') }]

# Numbers as Rails reads them (`to_d`); every row opens at the start's minute, so one page.
brow = ->(**over) { r = bn_row(minute * 1000, 0); over.each { |i, v| r[i.to_s.delete('f').to_i] = v }; r }
{ 'numbers_as_json_numbers' => brow.call(f1: 64_000.5, f2: 64_100, f3: 63_900.25, f4: 64_050, f5: 2.5),
  'float_digits' => brow.call(f1: 1.2345678901234567, f5: 1e-07),
  'many_digits' => brow.call(f1: '64000.123456789012345678901234567890', f5: '0.00000001'),
  'exponent_strings' => brow.call(f1: '6.4e4', f2: '6.41E4', f3: '6.39e4', f4: '6.4e4', f5: '1e-3'),
  'open_null' => brow.call(f1: nil),
  'open_word' => brow.call(f1: 'x'),
  'close_blank' => brow.call(f4: ''),
  'volume_null' => brow.call(f5: nil),
  'short_row' => [minute * 1000, '1', '2', '0.5', '1.5'],
  'time_string' => brow.call(f0: (minute * 1000).to_s),
  'time_float' => brow.call(f0: (minute * 1000) + 0.5),
  'time_null' => brow.call(f0: nil) }.each do |name, r|
  binance << ["row_#{name}", '1h', start, { b.call => ok([r]) }]
end
binance << ['row_not_an_array', '1h', start, { b.call => ok(['x']) }]

# Binance.US: the same method at its own host (its failures name it).
BINANCE_US_CASES = %w[1d_normal 1M_normal 1h_two_pages 1h_page_then_500 error_invalid_symbol http_500_empty http_429 not_json
                      transport_pre_send row_open_null].freeze
binance_us = binance.select { |c| BINANCE_US_CASES.include?(c[0]) }

# ---- Alpaca: one bars read (Clients::Alpaca#each_bars_page: limit 10000, every `next_page_token` followed), crypto by
# its pair under `symbols`, a stock by its code in the path with adjustment=split (#get_indicator_candles); 3 days are
# daily bars built.
AL_CRYPTO = { 'ticker' => 'BTC/USD', 'base' => 'BTC', 'crypto' => true }.freeze
AL_STOCK = { 'ticker' => 'AAPL', 'base' => 'AAPL', 'crypto' => false }.freeze
AL_TF = { '1m' => '1Min', '5m' => '5Min', '15m' => '15Min', '30m' => '30Min', '1h' => '1Hour', '4h' => '4Hour', '1d' => '1Day',
          '3d' => '1Day', '1w' => '1Week', '1M' => '1Month' }.freeze
def al_key(t, tf, start_at, token = nil)
  q = { 'limit' => '10000', 'start' => start_at.iso8601, 'timeframe' => AL_TF.fetch(tf), 'page_token' => token }.compact
  path = if t['crypto']
           q['symbols'] = t['ticker']
           '/v1beta3/crypto/us/bars'
         else
           q['adjustment'] = 'split'
           "/v2/stocks/#{t['base']}/bars"
         end
  "GET #{path}?#{q.sort.map { |k, v| "#{k}=#{v}" }.join('&')}"
end
# Alpaca sends numbers.
def al_bar(at, i)
  { 't' => Time.at(at).utc.iso8601, 'o' => 64_000.5 + (i * 37 % 101), 'h' => 64_200.25 + (i * 53 % 97), 'l' => 63_800.125 - (i * 29 % 89),
    'c' => 64_000.75 + (i * 71 % 103), 'v' => 0.0123 + i, 'n' => 10 + i, 'vw' => 64_001.1 }
end
def al_bars(times) = times.each_with_index.map { |at, i| al_bar(at, i) }
def al_body(t, bars, token = nil)
  t['crypto'] ? { 'bars' => { t['ticker'] => bars }, 'next_page_token' => token } : { 'bars' => bars, 'symbol' => t['base'], 'next_page_token' => token }
end

alpaca = []
[['crypto', AL_CRYPTO], ['stock', AL_STOCK]].each do |kind, t|
  TIMEFRAMES.each do |tf, duration|
    step = tf == '3d' ? 86_400 : duration.to_i
    times = (0...40).map { |i| minute - ((39 - i) * step) }
    alpaca << ["#{kind}_#{tf}_normal", tf, start, { al_key(t, tf, start) => ok(al_body(t, al_bars(times))) }, t]
    alpaca << ["#{kind}_#{tf}_empty", tf, start, { al_key(t, tf, start) => ok(al_body(t, [])) }, t]
    alpaca << ["#{kind}_#{tf}_http_500", tf, start, { al_key(t, tf, start) => status(500, '') }, t]
  end
  # 3 days from daily bars, epoch-aligned; a group before the first bar is dropped.
  [Time.utc(2026, 7, 1), Time.utc(2026, 7, 2)].each do |from|
    alpaca << ["#{kind}_3d_built_from_#{from.strftime('%Y%m%d')}", '3d', from,
               { al_key(t, '3d', from) => ok(al_body(t, al_bars((0...20).map { |i| from.to_i + (i * 86_400) }))) }, t]
  end
  # Pages joined in order; a blank token ends the walk.
  alpaca << ["#{kind}_two_pages", '1d', deep, { al_key(t, '1d', deep) => ok(al_body(t, al_bars([deep.to_i, deep.to_i + 86_400]), 'p2')),
                                                 al_key(t, '1d', deep, 'p2') => ok(al_body(t, al_bars([deep.to_i + 172_800]))) }, t]
  alpaca << ["#{kind}_three_pages_3d_built", '3d', deep,
             { al_key(t, '3d', deep) => ok(al_body(t, al_bars([deep.to_i, deep.to_i + 86_400]), 'p2')),
               al_key(t, '3d', deep, 'p2') => ok(al_body(t, al_bars([deep.to_i + 172_800, deep.to_i + 259_200]), 'p3')),
               al_key(t, '3d', deep, 'p3') => ok(al_body(t, al_bars([deep.to_i + 345_600]), '')) }, t]
  alpaca << ["#{kind}_empty_page_with_token", '1d', deep, { al_key(t, '1d', deep) => ok(al_body(t, [], 'p2')),
                                                             al_key(t, '1d', deep, 'p2') => ok(al_body(t, al_bars([deep.to_i]))) }, t]
  alpaca << ["#{kind}_page_then_500", '1d', deep, { al_key(t, '1d', deep) => ok(al_body(t, al_bars([deep.to_i]), 'p2')),
                                                     al_key(t, '1d', deep, 'p2') => status(500, { 'message' => 'internal error' }) }, t]
  alpaca << ["#{kind}_bars_null", '1d', start, { al_key(t, '1d', start) => ok(t['crypto'] ? { 'bars' => { t['ticker'] => nil } } : { 'bars' => nil }) }, t]
  alpaca << ["#{kind}_bars_missing", '1d', start, { al_key(t, '1d', start) => ok({ 'next_page_token' => nil }) }, t]
  alpaca << ["#{kind}_transport_pre_send", '1d', start, { al_key(t, '1d', start) => network('pre_send') }, t]
  alpaca << ["#{kind}_transport_timeout", '1d', start, { al_key(t, '1d', start) => network('post_send') }, t]
  alpaca << ["#{kind}_http_404", '1d', start, { al_key(t, '1d', start) => status(404, { 'message' => 'not found' }) }, t]
end

c = ->(tf = '1h') { al_key(AL_CRYPTO, tf, start) }
crypto_case = ->(name, reply) { alpaca << ["crypto_#{name}", '1h', start, { c.call => reply }, AL_CRYPTO] }
crypto_case.call('other_symbol_only', ok({ 'bars' => { 'ETH/USD' => al_bars([minute]) }, 'next_page_token' => nil }))
crypto_case.call('error_invalid_symbol', status(400, { 'message' => 'invalid symbol: BTC/USD' }))
crypto_case.call('error_unprocessable', status(422, { 'code' => 42_210_000, 'message' => 'invalid start' }))
crypto_case.call('http_401', status(401, { 'message' => 'unauthorized.' }))
crypto_case.call('http_403', status(403, { 'message' => 'forbidden.' }))
crypto_case.call('http_429', status(429, { 'message' => 'too many requests.' }))
crypto_case.call('http_500_text', status(500, 'upstream failed'))
crypto_case.call('http_502', status(502, '<html>bad gateway</html>'))
crypto_case.call('not_json', ok('<html>'))
crypto_case.call('json_array', ok([1, 2]))

bar = ->(**over) { al_bar(minute, 0).merge(over.transform_keys(&:to_s)) }
{ 'numbers_as_strings' => bar.call(o: '64000.5', h: '64100', l: '63900.25', c: '64050', v: '2.5'),
  'exponent_numbers' => bar.call(o: 6.4e4, v: 1e-07),
  'open_null' => bar.call(o: nil),
  'open_word' => bar.call(o: 'x'),
  'close_blank' => bar.call(c: ''),
  'volume_null' => bar.call(v: nil),
  'volume_missing' => al_bar(minute, 0).except('v'),
  'time_fraction' => bar.call(t: '2026-09-02T10:00:00.5Z'),
  'time_offset' => bar.call(t: '2026-09-02T12:00:00+02:00'),
  'time_null' => bar.call(t: nil),
  'time_word' => bar.call(t: 'soon'),
  'not_an_object' => 'x' }.each do |name, r|
  crypto_case.call("bar_#{name}", ok(al_body(AL_CRYPTO, [r])))
end
crypto_case.call('bars_not_an_array', ok({ 'bars' => { AL_CRYPTO['ticker'] => 'x' } }))
alpaca << ['stock_bars_not_an_array', '1h', start, { al_key(AL_STOCK, '1h', start) => ok({ 'bars' => 'x' }) }, AL_STOCK]

# ---- The other venues. Each row is built in the venue's own shape from one set of prices (`shaped`), and each venue
# gets the same numbers cases (`number_cases`).
def prices(i)
  { o: "#{64_000 + (i * 37 % 101)}.5", h: "#{64_200 + (i * 53 % 97)}.25", l: "#{63_800 - (i * 29 % 89)}.125",
    c: "#{64_000 + (i * 71 % 103)}.75", v: "#{(i % 5) + 1}.0001" }
end
# A row as an array in `order` (symbols are the time and prices, anything else is sent as it is) or as a hash of them.
def shaped(order, t, i, over)
  p = prices(i).merge(over)
  pick = ->(k) { k == :t ? t : (k.is_a?(Symbol) ? p[k] : k) }
  order.is_a?(Hash) ? order.transform_values(&pick) : order.map(&pick)
end
def forty(step, last) = (0...40).map { |i| last - ((39 - i) * step) }
# t0: the venue's own time value for the start's minute.
def number_cases(row, t0)
  float = t0.is_a?(String) ? "#{t0}.5" : t0 + 0.5
  cases = { 'numbers_as_json_numbers' => row.call(t0, 0, o: 64_000.5, h: 64_100, l: 63_900.25, c: 64_050, v: 2.5),
            'float_digits' => row.call(t0, 0, o: 1.2345678901234567, v: 1e-07),
            'many_digits' => row.call(t0, 0, o: '64000.123456789012345678901234567890', v: '0.00000001'),
            'exponent_strings' => row.call(t0, 0, o: '6.4e4', h: '6.41E4', l: '6.39e4', c: '6.4e4', v: '1e-3'),
            'open_null' => row.call(t0, 0, o: nil), 'open_word' => row.call(t0, 0, o: 'x'), 'close_blank' => row.call(t0, 0, c: ''),
            'volume_null' => row.call(t0, 0, v: nil), 'time_string' => row.call(t0.to_s, 0), 'time_number' => row.call(t0.to_i, 0),
            'time_float' => row.call(float, 0), 'time_null' => row.call(nil, 0), 'time_word' => row.call('soon', 0) }
  r = row.call(t0, 0)
  cases['short_row'] = r.first(5) if r.is_a?(Array)
  cases
end
def failure_cases(key, error_body)
  { 'http_400' => status(400, error_body), 'http_500_empty' => status(500, ''), 'http_502' => status(502, '<html>bad gateway</html>'),
    'http_401' => status(401, { 'msg' => 'unauthorized' }), 'http_403' => status(403, ''), 'http_429' => status(429, 'too many requests'),
    'not_json' => ok('<html>'), 'json_array' => ok([1, 2]), 'json_object' => ok({ 'code' => 1, 'msg' => 'odd' }), 'json_null' => ok(nil),
    'transport_pre_send' => network('pre_send'), 'transport_timeout' => network('post_send') }.map { |name, reply| [name, { key => reply }] }
end
NUMBER_TF = '1h'.freeze

# ---- KuCoin: windows of 1500 periods from the start (startAt/endAt in seconds) to now, at most MAX_CANDLE_PAGES; each
# window sorted, the next from a second after its latest candle (after the window's end when it had none); deduped and
# sorted; 3 days built from daily candles and 1 month from weekly. Rows are [time, open, close, high, low, volume, turnover].
KC_TYPE = { '1m' => '1min', '5m' => '5min', '15m' => '15min', '30m' => '30min', '1h' => '1hour', '4h' => '4hour', '1d' => '1day',
            '3d' => '1day', '1w' => '1week', '1M' => '1week' }.freeze
NATIVE_STEP = ->(tf, over = {}) { over.fetch(tf, TIMEFRAMES.fetch(tf).to_i) }
def kc_key(tf, from, to) = "GET /api/v1/market/candles?endAt=#{to.to_i}&startAt=#{from.to_i}&symbol=BTC-USDT&type=#{KC_TYPE.fetch(tf)}"
kc_row = ->(t, i, **over) { shaped([:t, :o, :c, :h, :l, :v, '64001.1'], t, i, over) }
kc_rows = ->(times) { times.each_with_index.map { |at, i| kc_row.call(at.to_s, i) }.reverse }
kc_body = ->(rows) { { 'code' => '200000', 'data' => rows } }
kucoin = []
TIMEFRAMES.each_key do |tf|
  times = forty(NATIVE_STEP.call(tf, { '3d' => 86_400, '1M' => 604_800 }), minute)
  key = kc_key(tf, start, NOW)
  kucoin << ["#{tf}_normal", tf, start, { key => ok(kc_body.call(kc_rows.call(times))) }]
  kucoin << ["#{tf}_gap", tf, start, { key => ok(kc_body.call(kc_rows.call(times.values_at(0..9, 11..20, 22..39)))) }]
  kucoin << ["#{tf}_empty", tf, start, { key => ok(kc_body.call([])) }]
  kucoin << ["#{tf}_http_500", tf, start, { key => status(500, 'upstream failed') }]
end
[['3d', Time.utc(2026, 7, 1), 86_400], ['3d', Time.utc(2026, 7, 2), 86_400], ['1M', Time.utc(2026, 6, 1), 604_800]].each do |tf, from, step|
  kucoin << ["#{tf}_built_from_#{from.strftime('%Y%m%d')}", tf, from,
             { kc_key(tf, from, NOW) => ok(kc_body.call(kc_rows.call((0...20).map { |i| from.to_i + (i * step) }))) }]
end
# Windows: 1500 minutes each.
w0 = Time.utc(2026, 9, 1, 8)
w1 = w0 + 1500.minutes
kucoin << ['1m_two_windows', '1m', w0, { kc_key('1m', w0, w1) => ok(kc_body.call(kc_rows.call([w1.to_i - 120, w1.to_i - 60]))),
                                         kc_key('1m', w1 - 59, NOW) => ok(kc_body.call(kc_rows.call([minute]))) }]
kucoin << ['1m_empty_window_skips_forward', '1m', w0, { kc_key('1m', w0, w1) => ok(kc_body.call([])),
                                                        kc_key('1m', w1 + 1, NOW) => ok(kc_body.call(kc_rows.call([minute]))) }]
kucoin << ['1m_window_then_500', '1m', w0, { kc_key('1m', w0, w1) => ok(kc_body.call(kc_rows.call([w1.to_i - 60]))),
                                             kc_key('1m', w1 - 59, NOW) => status(500, 'upstream failed') }]
kucoin << ['1m_overlap_deduped', '1m', w0, { kc_key('1m', w0, w1) => ok(kc_body.call(kc_rows.call([w1.to_i - 120, w1.to_i - 60]))),
                                             kc_key('1m', w1 - 59, NOW) => ok(kc_body.call([kc_row.call((w1.to_i - 60).to_s, 7), kc_row.call(minute.to_s, 8)])) }]
kucoin << ['1m_window_order_as_sent', '1m', w0, { kc_key('1m', w0, w1) => ok(kc_body.call(kc_rows.call([w1.to_i - 60, w1.to_i - 120]).reverse)),
                                                  kc_key('1m', w1 - 59, NOW) => ok(kc_body.call([])) }]
v0 = w0 - 1500.minutes
kucoin << ['1m_three_windows', '1m', v0, { kc_key('1m', v0, w0) => ok(kc_body.call([])),
                                           kc_key('1m', w0 + 1, w0 + 1 + 1500.minutes) => ok(kc_body.call(kc_rows.call([(w0 + 1500.minutes).to_i - 60]))),
                                           kc_key('1m', w0 + 1500.minutes - 59, NOW) => ok(kc_body.call(kc_rows.call([minute]))) }]
# The 20-year walk: empty windows skipped to the one with candles.
walk_from = Time.utc(2006, 9, 1)
walk = {}
loop do
  to = [walk_from + 1500.days, NOW].min
  walk[kc_key('1d', walk_from, to)] = ok(kc_body.call(to == NOW ? kc_rows.call([minute - (minute % 86_400)]) : []))
  break if to == NOW

  walk_from = to + 1
end
kucoin << ['1d_twenty_years', '1d', Time.utc(2006, 9, 1), walk]
k = kc_key(NUMBER_TF, start, NOW)
failure_cases(k, { 'code' => '400100', 'msg' => 'This pair is not provided at present' }).each { |name, http| kucoin << [name, NUMBER_TF, start, http] }
kucoin << ['error_envelope', NUMBER_TF, start, { k => ok({ 'code' => '400100', 'msg' => 'This pair is not provided at present' }) }]
kucoin << ['data_null', NUMBER_TF, start, { k => ok({ 'code' => '200000', 'data' => nil }) }]
kucoin << ['data_not_an_array', NUMBER_TF, start, { k => ok({ 'code' => '200000', 'data' => 'x' }) }]
number_cases(kc_row, minute.to_s).each { |name, r| kucoin << ["row_#{name}", NUMBER_TF, start, { k => ok(kc_body.call([r])) }] }

# ---- Bitget: pages of up to 1000 from startTime (ms) to now; a full page asks again from a second after its last row
# (as sent); a blank page ends; 3 days built from daily. Rows are [time, open, high, low, close, volume, usdt, quote].
BG_GRAN = { '1m' => '1min', '5m' => '5min', '15m' => '15min', '30m' => '30min', '1h' => '1h', '4h' => '4h', '1d' => '1day',
            '3d' => '1day', '1w' => '1Wutc', '1M' => '1Mutc' }.freeze
def bg_key(tf, from) = "GET /api/v2/spot/market/candles?endTime=#{NOW.to_i * 1000}&granularity=#{BG_GRAN.fetch(tf)}&limit=1000&startTime=#{from.to_i * 1000}&symbol=BTCUSDT"
bg_row = ->(t, i, **over) { shaped([:t, :o, :h, :l, :c, :v, '64001.1', '64001.1'], t, i, over) }
bg_rows = ->(times) { times.each_with_index.map { |at, i| bg_row.call((at * 1000).to_s, i) } }
bg_body = ->(rows) { { 'code' => '00000', 'msg' => 'success', 'requestTime' => NOW.to_i * 1000, 'data' => rows } }
bitget = []
TIMEFRAMES.each_key do |tf|
  times = forty(NATIVE_STEP.call(tf, { '3d' => 86_400 }), minute)
  key = bg_key(tf, start)
  bitget << ["#{tf}_normal", tf, start, { key => ok(bg_body.call(bg_rows.call(times))) }]
  bitget << ["#{tf}_gap", tf, start, { key => ok(bg_body.call(bg_rows.call(times.values_at(0..9, 11..20, 22..39)))) }]
  bitget << ["#{tf}_empty", tf, start, { key => ok(bg_body.call([])) }]
  bitget << ["#{tf}_http_500", tf, start, { key => status(500, 'upstream failed') }]
end
[Time.utc(2026, 7, 1), Time.utc(2026, 7, 2)].each do |from|
  bitget << ["3d_built_from_#{from.strftime('%Y%m%d')}", '3d', from,
             { bg_key('3d', from) => ok(bg_body.call(bg_rows.call((0...20).map { |i| from.to_i + (i * 86_400) }))) }]
end
p0 = minute - (1100 * 60)
full = (0...1000).map { |i| p0 + (i * 60) }
bitget << ['1m_full_page_then_rest', '1m', Time.at(p0).utc, { bg_key('1m', Time.at(p0)) => ok(bg_body.call(bg_rows.call(full))),
                                                              bg_key('1m', Time.at(full.last + 1)) => ok(bg_body.call(bg_rows.call((1000...1003).map { |i| p0 + (i * 60) }))) }]
bitget << ['1m_full_page_then_empty', '1m', Time.at(p0).utc, { bg_key('1m', Time.at(p0)) => ok(bg_body.call(bg_rows.call(full))),
                                                               bg_key('1m', Time.at(full.last + 1)) => ok(bg_body.call([])) }]
bitget << ['1m_full_page_then_500', '1m', Time.at(p0).utc, { bg_key('1m', Time.at(p0)) => ok(bg_body.call(bg_rows.call(full))),
                                                             bg_key('1m', Time.at(full.last + 1)) => status(500, '') }]
# The next page follows the LAST row as sent, not the latest.
bitget << ['1m_full_page_newest_first', '1m', Time.at(p0).utc, { bg_key('1m', Time.at(p0)) => ok(bg_body.call(bg_rows.call(full).reverse)),
                                                                 bg_key('1m', Time.at(p0 + 1)) => ok(bg_body.call([])) }]
k = bg_key(NUMBER_TF, start)
failure_cases(k, { 'code' => '40034', 'msg' => 'Parameter verification exception', 'requestTime' => 1, 'data' => nil }).each { |name, http| bitget << [name, NUMBER_TF, start, http] }
bitget << ['error_envelope', NUMBER_TF, start, { k => ok({ 'code' => '40034', 'msg' => 'Parameter verification exception', 'data' => nil }) }]
bitget << ['data_not_an_array', NUMBER_TF, start, { k => ok({ 'code' => '00000', 'data' => 'x' }) }]
number_cases(bg_row, (minute * 1000).to_s).each { |name, r| bitget << ["row_#{name}", NUMBER_TF, start, { k => ok(bg_body.call([r])) }] }

# ---- Bybit: pages of up to 1000 from `start` (ms), newest first, each read oldest first; a full page asks again from a
# second after its latest; a retCode other than 0 fails with retMsg; 3 days built from daily.
BY_INT = { '1m' => '1', '5m' => '5', '15m' => '15', '30m' => '30', '1h' => '60', '4h' => '240', '1d' => 'D', '3d' => 'D', '1w' => 'W',
           '1M' => 'M' }.freeze
def by_key(tf, from) = "GET /v5/market/kline?category=spot&interval=#{BY_INT.fetch(tf)}&limit=1000&start=#{from.to_i * 1000}&symbol=BTCUSDT"
by_row = ->(t, i, **over) { shaped([:t, :o, :h, :l, :c, :v, '64001.1'], t, i, over) }
by_rows = ->(times) { times.each_with_index.map { |at, i| by_row.call((at * 1000).to_s, i) }.reverse }
by_body = ->(rows) { { 'retCode' => 0, 'retMsg' => 'OK', 'result' => { 'category' => 'spot', 'symbol' => 'BTCUSDT', 'list' => rows }, 'retExtInfo' => {}, 'time' => 1 } }
bybit = []
TIMEFRAMES.each_key do |tf|
  times = forty(NATIVE_STEP.call(tf, { '3d' => 86_400 }), minute)
  key = by_key(tf, start)
  bybit << ["#{tf}_normal", tf, start, { key => ok(by_body.call(by_rows.call(times))) }]
  bybit << ["#{tf}_gap", tf, start, { key => ok(by_body.call(by_rows.call(times.values_at(0..9, 11..20, 22..39)))) }]
  bybit << ["#{tf}_empty", tf, start, { key => ok(by_body.call([])) }]
  bybit << ["#{tf}_http_500", tf, start, { key => status(500, 'upstream failed') }]
end
[Time.utc(2026, 7, 1), Time.utc(2026, 7, 2)].each do |from|
  bybit << ["3d_built_from_#{from.strftime('%Y%m%d')}", '3d', from,
            { by_key('3d', from) => ok(by_body.call(by_rows.call((0...20).map { |i| from.to_i + (i * 86_400) }))) }]
end
bybit << ['1m_full_page_then_rest', '1m', Time.at(p0).utc, { by_key('1m', Time.at(p0)) => ok(by_body.call(by_rows.call(full))),
                                                             by_key('1m', Time.at(full.last + 1)) => ok(by_body.call(by_rows.call((1000...1003).map { |i| p0 + (i * 60) }))) }]
bybit << ['1m_full_page_then_retcode', '1m', Time.at(p0).utc, { by_key('1m', Time.at(p0)) => ok(by_body.call(by_rows.call(full))),
                                                                by_key('1m', Time.at(full.last + 1)) => ok({ 'retCode' => 10_006, 'retMsg' => 'Too many visits!' }) }]
k = by_key(NUMBER_TF, start)
failure_cases(k, { 'retCode' => 10_001, 'retMsg' => 'Not supported symbols', 'result' => {} }).each { |name, http| bybit << [name, NUMBER_TF, start, http] }
bybit << ['http_400_msg', NUMBER_TF, start, { k => status(400, { 'msg' => 'bad request' }) }]
bybit << ['retcode_error', NUMBER_TF, start, { k => ok({ 'retCode' => 10_001, 'retMsg' => 'Not supported symbols', 'result' => {} }) }]
bybit << ['retcode_string', NUMBER_TF, start, { k => ok({ 'retCode' => '0', 'retMsg' => 'OK', 'result' => { 'list' => [] } }) }]
bybit << ['result_missing', NUMBER_TF, start, { k => ok({ 'retCode' => 0, 'retMsg' => 'OK' }) }]
bybit << ['list_null', NUMBER_TF, start, { k => ok({ 'retCode' => 0, 'retMsg' => 'OK', 'result' => { 'list' => nil } }) }]
bybit << ['list_not_an_array', NUMBER_TF, start, { k => ok({ 'retCode' => 0, 'retMsg' => 'OK', 'result' => { 'list' => 'x' } }) }]
number_cases(by_row, (minute * 1000).to_s).each { |name, r| bybit << ["row_#{name}", NUMBER_TF, start, { k => ok(by_body.call([r])) }] }

# ---- MEXC: the range from the start's whole second to now in windows of 500 periods (3 days: 500 days), each its own
# read, joined in order; a failure's `msg` when it has one; 3 days built from daily.
MX_INT = { '1m' => '1m', '5m' => '5m', '15m' => '15m', '30m' => '30m', '1h' => '60m', '4h' => '4h', '1d' => '1d', '3d' => '1d',
           '1w' => '1W', '1M' => '1M' }.freeze
def mx_span(tf) = 500 * (tf == '3d' ? 86_400 : TIMEFRAMES.fetch(tf).to_i) * 1000
def mx_key(tf, from_ms) = "GET /api/v3/klines?endTime=#{from_ms + mx_span(tf) - 1}&interval=#{MX_INT.fetch(tf)}&limit=500&startTime=#{from_ms}&symbol=BTCUSDT"
mx_row = ->(t, i, **over) { shaped([:t, :o, :h, :l, :c, :v, 0, '64001.1'], t, i, over) }
mx_rows = ->(times) { times.each_with_index.map { |at, i| mx_row.call(at * 1000, i) } }
mexc = []
TIMEFRAMES.each_key do |tf|
  times = forty(NATIVE_STEP.call(tf, { '3d' => 86_400 }), minute)
  key = mx_key(tf, start.to_i * 1000)
  mexc << ["#{tf}_normal", tf, start, { key => ok(mx_rows.call(times)) }]
  mexc << ["#{tf}_gap", tf, start, { key => ok(mx_rows.call(times.values_at(0..9, 11..20, 22..39))) }]
  mexc << ["#{tf}_empty", tf, start, { key => ok([]) }]
  mexc << ["#{tf}_http_500", tf, start, { key => status(500, 'upstream failed') }]
end
windows = ->(tf, from) { (from.to_i * 1000).step(NOW.to_i * 1000, mx_span(tf)).to_a }
two = windows.call('1h', deep)
mexc << ['1h_two_windows', '1h', deep, { mx_key('1h', two[0]) => ok(mx_rows.call([deep.to_i, deep.to_i + 3600])), mx_key('1h', two[1]) => ok(mx_rows.call([minute])) }]
mexc << ['1h_window_then_error', '1h', deep, { mx_key('1h', two[0]) => ok(mx_rows.call([deep.to_i])),
                                               mx_key('1h', two[1]) => status(400, { 'code' => -1121, 'msg' => 'Invalid symbol.' }) }]
mexc << ['1h_window_then_transport', '1h', deep, { mx_key('1h', two[0]) => ok(mx_rows.call([deep.to_i])), mx_key('1h', two[1]) => network('post_send') }]
built = windows.call('3d', Time.utc(2025, 1, 1))
mexc << ['3d_built_across_windows', '3d', Time.utc(2025, 1, 1),
         { mx_key('3d', built[0]) => ok(mx_rows.call((0...5).map { |i| Time.utc(2025, 1, 1).to_i + (i * 86_400) })),
           mx_key('3d', built[1]) => ok(mx_rows.call((0...5).map { |i| (built[1] / 1000) + (i * 86_400) })) }]
mexc << ['3d_built_empty', '3d', start, { mx_key('3d', start.to_i * 1000) => ok([]) }]
years = windows.call('1d', Time.utc(2006, 9, 1))
mexc << ['1d_twenty_years', '1d', Time.utc(2006, 9, 1), years.to_h { |ws| [mx_key('1d', ws), ok(ws == years.last ? mx_rows.call([minute - (minute % 86_400)]) : [])] }]
# A window from exactly now is still read.
mexc << ['1m_window_from_now', '1m', NOW, { mx_key('1m', NOW.to_i * 1000) => ok(mx_rows.call([NOW.to_i - 1])) }]
k = mx_key(NUMBER_TF, start.to_i * 1000)
failure_cases(k, { 'code' => -1121, 'msg' => 'Invalid symbol.' }).each { |name, http| mexc << [name, NUMBER_TF, start, http] }
mexc << ['http_400_blank_msg', NUMBER_TF, start, { k => status(400, { 'code' => 1, 'msg' => '' }) }]
mexc << ['http_400_no_msg', NUMBER_TF, start, { k => status(400, { 'code' => 1 }) }]
number_cases(mx_row, minute * 1000).each { |name, r| mexc << ["row_#{name}", NUMBER_TF, start, { k => ok([r]) }] }

# ---- Bitrue: the newest 1440 rows, then older pages before the oldest (`fromIdx`) until a short page or one reaching
# the start; deduped by `is`, sorted, 3 days and 1 month built from daily (none to build from is a failure), then only
# the candles from the start on.
BT_SCALE = { '1m' => '1m', '5m' => '5m', '15m' => '15m', '30m' => '30m', '1h' => '1h', '4h' => '4h', '1d' => '1d', '3d' => '1d',
             '1w' => '1w', '1M' => '1d' }.freeze
def bt_key(tf, from_idx = nil) = "GET /api/v1/market/kline?#{"fromIdx=#{from_idx}&" if from_idx}limit=1440&scale=#{BT_SCALE.fetch(tf)}&symbol=BTCUSDT"
bt_row = ->(t, i, **over) { shaped({ 'i' => 1, 'is' => :t, 'a' => '64001.1', 'v' => :v, 'c' => :c, 'h' => :h, 'l' => :l, 'o' => :o }, t, i, over) }
bt_rows = ->(times) { times.each_with_index.map { |at, i| bt_row.call(at * 1000, i) }.reverse }
bt_body = ->(rows) { { 'symbol' => 'BTCUSDT', 'scale' => '1h', 'data' => rows } }
bitrue = []
TIMEFRAMES.each_key do |tf|
  times = forty(NATIVE_STEP.call(tf, { '3d' => 86_400, '1M' => 86_400 }), minute)
  from = Time.at(times[20]).utc
  bitrue << ["#{tf}_normal", tf, from, { bt_key(tf) => ok(bt_body.call(bt_rows.call(times))) }]
  bitrue << ["#{tf}_gap", tf, from, { bt_key(tf) => ok(bt_body.call(bt_rows.call(times.values_at(0..9, 11..20, 22..39)))) }]
  bitrue << ["#{tf}_empty", tf, from, { bt_key(tf) => ok(bt_body.call([])) }]
  bitrue << ["#{tf}_http_500", tf, from, { bt_key(tf) => status(500, 'upstream failed') }]
end
[Time.utc(2026, 7, 1), Time.utc(2026, 7, 2)].each do |from|
  bitrue << ["3d_built_from_#{from.strftime('%Y%m%d')}", '3d', from,
             { bt_key('3d') => ok(bt_body.call(bt_rows.call((0...20).map { |i| from.to_i + (i * 86_400) }))) }]
end
bitrue << ['1M_built_from_20260615', '1M', Time.utc(2026, 6, 15), { bt_key('1M') => ok(bt_body.call(bt_rows.call((0...75).map { |i| Time.utc(2026, 6, 1).to_i + (i * 86_400) }))) }]
b0 = minute - (1500 * 60)
newest = (60...1500).map { |i| b0 + (i * 60) }
oldest_ms = newest.first * 1000
bitrue << ['1m_full_page_then_older', '1m', Time.at(b0).utc, { bt_key('1m') => ok(bt_body.call(bt_rows.call(newest))),
                                                               bt_key('1m', oldest_ms - 1) => ok(bt_body.call(bt_rows.call((0...61).map { |i| b0 + (i * 60) }))) }]
bitrue << ['1m_full_page_reaches_the_start', '1m', Time.at(newest.first).utc, { bt_key('1m') => ok(bt_body.call(bt_rows.call(newest))) }]
bitrue << ['1m_full_page_then_newer_ignored', '1m', Time.at(b0).utc, { bt_key('1m') => ok(bt_body.call(bt_rows.call(newest))),
                                                                       bt_key('1m', oldest_ms - 1) => ok(bt_body.call(bt_rows.call([minute]))) }]
bitrue << ['1m_full_page_then_empty', '1m', Time.at(b0).utc, { bt_key('1m') => ok(bt_body.call(bt_rows.call(newest))),
                                                               bt_key('1m', oldest_ms - 1) => ok(bt_body.call([])) }]
bitrue << ['1m_full_page_then_error', '1m', Time.at(b0).utc, { bt_key('1m') => ok(bt_body.call(bt_rows.call(newest))),
                                                               bt_key('1m', oldest_ms - 1) => status(400, { 'code' => -1121, 'msg' => 'Invalid symbol.' }) }]
bitrue << ['1m_duplicate_kept_first', '1m', Time.at(minute - 120).utc, { bt_key('1m') => ok(bt_body.call([bt_row.call(minute * 1000, 1), bt_row.call(minute * 1000, 2), bt_row.call((minute - 60) * 1000, 3)])) }]
bitrue << ['1m_start_kept_with_its_fraction', '1m', Time.at(minute, 500, :millisecond).utc, { bt_key('1m') => ok(bt_body.call(bt_rows.call([minute - 60, minute, minute + 60]))) }]
k = bt_key(NUMBER_TF)
failure_cases(k, { 'code' => -1121, 'msg' => 'Invalid symbol.' }).each { |name, http| bitrue << [name, NUMBER_TF, Time.at(minute - 3600).utc, http] }
bitrue << ['data_null', NUMBER_TF, start, { k => ok({ 'data' => nil }) }]
number_cases(bt_row, minute * 1000).each { |name, r| bitrue << ["row_#{name}", NUMBER_TF, Time.at(minute - 3600).utc, { k => ok(bt_body.call([r])) }] }

# ---- Bitvavo: the newest 1440 of the range from the start (ms), then older pages ending at the oldest, until a short
# page or one no older; keyed by time (a later row replacing an earlier), sorted; 3 days, 1 week and 1 month built from daily.
BV_INT = { '1m' => '1m', '5m' => '5m', '15m' => '15m', '30m' => '30m', '1h' => '1h', '4h' => '4h', '1d' => '1d', '3d' => '1d',
           '1w' => '1d', '1M' => '1d' }.freeze
def bv_key(tf, from, end_ms = nil) = "GET /v2/BTC-EUR/candles?#{"end=#{end_ms}&" if end_ms}interval=#{BV_INT.fetch(tf)}&limit=1440&start=#{from.to_i * 1000}"
bv_row = ->(t, i, **over) { shaped([:t, :o, :h, :l, :c, :v], t, i, over) }
bv_rows = ->(times) { times.each_with_index.map { |at, i| bv_row.call(at * 1000, i) }.reverse }
bitvavo = []
TIMEFRAMES.each_key do |tf|
  times = forty(NATIVE_STEP.call(tf, { '3d' => 86_400, '1w' => 86_400, '1M' => 86_400 }), minute)
  key = bv_key(tf, start)
  bitvavo << ["#{tf}_normal", tf, start, { key => ok(bv_rows.call(times)) }]
  bitvavo << ["#{tf}_gap", tf, start, { key => ok(bv_rows.call(times.values_at(0..9, 11..20, 22..39))) }]
  bitvavo << ["#{tf}_empty", tf, start, { key => ok([]) }]
  bitvavo << ["#{tf}_http_500", tf, start, { key => status(500, 'upstream failed') }]
end
[['3d', Time.utc(2026, 7, 1)], ['3d', Time.utc(2026, 7, 2)], ['1w', Time.utc(2026, 7, 1)], ['1M', Time.utc(2026, 6, 15)]].each do |tf, from|
  bitvavo << ["#{tf}_built_from_#{from.strftime('%Y%m%d')}", tf, from, { bv_key(tf, from) => ok(bv_rows.call((0...75).map { |i| from.to_i + (i * 86_400) })) }]
end
vb = Time.at(b0).utc
bitvavo << ['1m_full_page_then_older', '1m', vb, { bv_key('1m', vb) => ok(bv_rows.call(newest)),
                                                   bv_key('1m', vb, oldest_ms) => ok(bv_rows.call((0...61).map { |i| b0 + (i * 60) })) }]
bitvavo << ['1m_full_page_then_no_older', '1m', vb, { bv_key('1m', vb) => ok(bv_rows.call(newest)), bv_key('1m', vb, oldest_ms) => ok(bv_rows.call([newest.first + 60])) }]
bitvavo << ['1m_full_page_then_empty', '1m', vb, { bv_key('1m', vb) => ok(bv_rows.call(newest)), bv_key('1m', vb, oldest_ms) => ok([]) }]
bitvavo << ['1m_full_page_then_500', '1m', vb, { bv_key('1m', vb) => ok(bv_rows.call(newest)), bv_key('1m', vb, oldest_ms) => status(500, '') }]
bitvavo << ['1m_duplicate_later_replaces', '1m', start, { bv_key('1m', start) => ok([bv_row.call(minute * 1000, 1), bv_row.call(minute * 1000, 2), bv_row.call((minute - 60) * 1000, 3)]) }]
k = bv_key(NUMBER_TF, start)
failure_cases(k, { 'errorCode' => 205, 'error' => 'market parameter is invalid.' }).each { |name, http| bitvavo << [name, NUMBER_TF, start, http] }
number_cases(bv_row, minute * 1000).each { |name, r| bitvavo << ["row_#{name}", NUMBER_TF, start, { k => ok([r]) }] }

# ---- Gemini: one recent window per native resolution, newest first; 4 hours built from hourly, 3 days, 1 week and
# 1 month from daily (none to build from is a failure); then only the candles from the start on.
GM_TF = { '1m' => '1m', '5m' => '5m', '15m' => '15m', '30m' => '30m', '1h' => '1hr', '4h' => '1hr', '1d' => '1day', '3d' => '1day',
          '1w' => '1day', '1M' => '1day' }.freeze
def gm_key(tf) = "GET /v2/candles/btcusd/#{GM_TF.fetch(tf)}"
gm_row = ->(t, i, **over) { shaped([:t, :o, :h, :l, :c, :v], t, i, over) }
gm_rows = ->(times) { times.each_with_index.map { |at, i| gm_row.call(at * 1000, i) }.reverse }
gemini = []
TIMEFRAMES.each_key do |tf|
  times = forty(NATIVE_STEP.call(tf, { '4h' => 3600, '3d' => 86_400, '1w' => 86_400, '1M' => 86_400 }), minute)
  from = Time.at(times[20]).utc
  gemini << ["#{tf}_normal", tf, from, { gm_key(tf) => ok(gm_rows.call(times)) }]
  gemini << ["#{tf}_gap", tf, from, { gm_key(tf) => ok(gm_rows.call(times.values_at(0..9, 11..20, 22..39))) }]
  gemini << ["#{tf}_empty", tf, from, { gm_key(tf) => ok([]) }]
  gemini << ["#{tf}_http_500", tf, from, { gm_key(tf) => status(500, 'upstream failed') }]
end
[['4h', Time.utc(2026, 9, 1), 3600], ['3d', Time.utc(2026, 7, 1), 86_400], ['1w', Time.utc(2026, 7, 6), 86_400], ['1M', Time.utc(2026, 6, 15), 86_400]].each do |tf, from, step|
  gemini << ["#{tf}_built_from_#{from.strftime('%Y%m%d')}", tf, from, { gm_key(tf) => ok(gm_rows.call((0...75).map { |i| from.to_i + ((i - 10) * step) })) }]
end
gemini << ['1m_start_kept_with_its_fraction', '1m', Time.at(minute, 500, :millisecond).utc, { gm_key('1m') => ok(gm_rows.call([minute - 60, minute, minute + 60])) }]
gemini << ['4h_built_none_from_the_start', '4h', NOW + 3600, { gm_key('4h') => ok(gm_rows.call([minute])) }]
k = gm_key(NUMBER_TF)
gfrom = Time.at(minute - 3600).utc
failure_cases(k, { 'result' => 'error', 'reason' => 'InvalidSymbol', 'message' => 'Supplied value is not a valid symbol' }).each { |name, http| gemini << [name, NUMBER_TF, gfrom, http] }
gemini << ['json_object_built', '4h', gfrom, { gm_key('4h') => ok({ 'result' => 'error' }) }]
number_cases(gm_row, minute * 1000).each { |name, r| gemini << ["row_#{name}", NUMBER_TF, gfrom, { k => ok([r]) }] }

# ---- Hyperliquid: one candleSnapshot from the start (ms) to now; 3 days, 1 week and 1 month built from daily.
HL_INT = { '1m' => '1m', '5m' => '5m', '15m' => '15m', '30m' => '30m', '1h' => '1h', '4h' => '4h', '1d' => '1d', '3d' => '1d', '1w' => '1d',
           '1M' => '1d' }.freeze
HL_COIN = '@107'.freeze
HL_KEY = 'POST /info'.freeze
hl_row = ->(t, i, **over) { shaped({ 't' => :t, 'T' => 0, 's' => HL_COIN, 'i' => '1h', 'o' => :o, 'c' => :c, 'h' => :h, 'l' => :l, 'v' => :v, 'n' => 10 }, t, i, over) }
hl_rows = ->(times) { times.each_with_index.map { |at, i| hl_row.call(at * 1000, i) } }
hyperliquid = []
TIMEFRAMES.each_key do |tf|
  times = forty(NATIVE_STEP.call(tf, { '3d' => 86_400, '1w' => 86_400, '1M' => 86_400 }), minute)
  hyperliquid << ["#{tf}_normal", tf, start, { HL_KEY => ok(hl_rows.call(times)) }]
  hyperliquid << ["#{tf}_gap", tf, start, { HL_KEY => ok(hl_rows.call(times.values_at(0..9, 11..20, 22..39))) }]
  hyperliquid << ["#{tf}_empty", tf, start, { HL_KEY => ok([]) }]
  hyperliquid << ["#{tf}_http_500", tf, start, { HL_KEY => status(500, 'upstream failed') }]
end
[['3d', Time.utc(2026, 7, 1)], ['3d', Time.utc(2026, 7, 2)], ['1w', Time.utc(2026, 7, 1)], ['1M', Time.utc(2026, 6, 15)]].each do |tf, from|
  hyperliquid << ["#{tf}_built_from_#{from.strftime('%Y%m%d')}", tf, from, { HL_KEY => ok(hl_rows.call((0...75).map { |i| from.to_i + (i * 86_400) })) }]
end
failure_cases(HL_KEY, 'Failed to deserialize the JSON body into the target type').each { |name, http| hyperliquid << [name, NUMBER_TF, start, http] }
number_cases(hl_row, minute * 1000).each { |name, r| hyperliquid << ["row_#{name}", NUMBER_TF, start, { HL_KEY => ok([r]) }] }
hyperliquid << ['row_volume_missing', NUMBER_TF, start, { HL_KEY => ok([hl_row.call(minute * 1000, 0).except('v')]) }]

# ---- Coinbase: windows of 350 periods (start/end in seconds) to now, each sorted; the next from a second after the
# latest candle so far (from the window's end while there is none); 4 hours built from two-hourly, 3 days, 1 week and
# 1 month from daily.
CB_GRAN = { '1m' => 'ONE_MINUTE', '5m' => 'FIVE_MINUTE', '15m' => 'FIFTEEN_MINUTE', '30m' => 'THIRTY_MINUTE', '1h' => 'ONE_HOUR',
            '4h' => 'TWO_HOUR', '1d' => 'ONE_DAY', '3d' => 'ONE_DAY', '1w' => 'ONE_DAY', '1M' => 'ONE_DAY' }.freeze
def cb_key(tf, from, to) = "GET /api/v3/brokerage/market/products/BTC-USD/candles?end=#{to.to_i}&granularity=#{CB_GRAN.fetch(tf)}&start=#{from.to_i}"
cb_row = ->(t, i, **over) { shaped({ 'start' => :t, 'low' => :l, 'high' => :h, 'open' => :o, 'close' => :c, 'volume' => :v }, t, i, over) }
cb_rows = ->(times) { times.each_with_index.map { |at, i| cb_row.call(at.to_s, i) }.reverse }
cb_body = ->(rows) { { 'candles' => rows } }
coinbase = []
TIMEFRAMES.each_key do |tf|
  times = forty(NATIVE_STEP.call(tf, { '4h' => 7200, '3d' => 86_400, '1w' => 86_400, '1M' => 86_400 }), minute)
  key = cb_key(tf, start, NOW)
  coinbase << ["#{tf}_normal", tf, start, { key => ok(cb_body.call(cb_rows.call(times))) }]
  coinbase << ["#{tf}_gap", tf, start, { key => ok(cb_body.call(cb_rows.call(times.values_at(0..9, 11..20, 22..39)))) }]
  coinbase << ["#{tf}_empty", tf, start, { key => ok(cb_body.call([])) }]
  coinbase << ["#{tf}_http_500", tf, start, { key => status(500, 'upstream failed') }]
end
[['4h', Time.utc(2026, 9, 1), 7200], ['3d', Time.utc(2026, 7, 1), 86_400], ['1w', Time.utc(2026, 7, 6), 86_400], ['1M', Time.utc(2026, 6, 15), 86_400]].each do |tf, from, step|
  coinbase << ["#{tf}_built_from_#{from.strftime('%Y%m%d')}", tf, from, { cb_key(tf, from, NOW) => ok(cb_body.call(cb_rows.call((0...60).map { |i| from.to_i + (i * step) }))) }]
end
c0 = NOW - 360.days
c1 = c0 + 350.days
coinbase << ['1d_two_windows', '1d', c0, { cb_key('1d', c0, c1) => ok(cb_body.call(cb_rows.call([c1.to_i - 172_800, c1.to_i - 86_400]))),
                                           cb_key('1d', c1 - 86_399, NOW) => ok(cb_body.call(cb_rows.call([minute - (minute % 86_400)]))) }]
coinbase << ['1d_empty_window_then_page', '1d', c0, { cb_key('1d', c0, c1) => ok(cb_body.call([])),
                                                      cb_key('1d', c1, NOW) => ok(cb_body.call(cb_rows.call([minute - (minute % 86_400)]))) }]
coinbase << ['1d_window_then_500', '1d', c0, { cb_key('1d', c0, c1) => ok(cb_body.call(cb_rows.call([c1.to_i - 86_400]))),
                                               cb_key('1d', c1 - 86_399, NOW) => status(500, '') }]
coinbase << ['1d_window_then_empty', '1d', c0, { cb_key('1d', c0, c1) => ok(cb_body.call(cb_rows.call([c1.to_i - 86_400]))),
                                                 cb_key('1d', c1 - 86_399, NOW) => ok(cb_body.call([])) }]
k = cb_key(NUMBER_TF, start, NOW)
failure_cases(k, { 'error' => 'INVALID_ARGUMENT', 'error_details' => 'start and end argument is invalid', 'message' => 'start and end argument is invalid' })
  .each { |name, http| coinbase << [name, NUMBER_TF, start, http] }
coinbase << ['candles_null', NUMBER_TF, start, { k => ok({ 'candles' => nil }) }]
number_cases(cb_row, minute.to_s).each { |name, r| coinbase << ["row_#{name}", NUMBER_TF, start, { k => ok(cb_body.call([r])) }] }

# ---- BingX: the newest 1000 at timeZone=0, rows before the start's whole second dropped, sorted; nothing built.
BX_INT = { '1m' => '1m', '5m' => '5m', '15m' => '15m', '30m' => '30m', '1h' => '1h', '4h' => '4h', '1d' => '1d', '3d' => '3d', '1w' => '1w',
           '1M' => '1M' }.freeze
def bx_key(tf) = "GET /openApi/spot/v1/market/kline?interval=#{BX_INT.fetch(tf)}&limit=1000&symbol=BTC-USDT&timeZone=0"
bx_row = ->(t, i, **over) { shaped([:t, :o, :h, :l, :c, :v, 0, '64001.1'], t, i, over) }
bx_rows = ->(times) { times.each_with_index.map { |at, i| bx_row.call(at * 1000, i) }.reverse }
bx_body = ->(rows) { { 'code' => 0, 'timestamp' => 1, 'data' => rows } }
bingx = []
TIMEFRAMES.each_key do |tf|
  times = forty(TIMEFRAMES.fetch(tf).to_i, minute)
  from = Time.at(times[20], 500, :millisecond).utc
  bingx << ["#{tf}_normal", tf, from, { bx_key(tf) => ok(bx_body.call(bx_rows.call(times))) }]
  bingx << ["#{tf}_gap", tf, from, { bx_key(tf) => ok(bx_body.call(bx_rows.call(times.values_at(0..9, 11..20, 22..39)))) }]
  bingx << ["#{tf}_empty", tf, from, { bx_key(tf) => ok(bx_body.call([])) }]
  bingx << ["#{tf}_http_500", tf, from, { bx_key(tf) => status(500, 'upstream failed') }]
end
k = bx_key(NUMBER_TF)
bfrom = Time.at(minute - 3600).utc
failure_cases(k, { 'code' => 100_400, 'msg' => 'Invalid parameters' }).each { |name, http| bingx << [name, NUMBER_TF, bfrom, http] }
bingx << ['error_envelope', NUMBER_TF, bfrom, { k => ok({ 'code' => 100_204, 'msg' => 'symbol not exist', 'timestamp' => 1 }) }]
bingx << ['bare_rows', NUMBER_TF, bfrom, { k => ok(bx_rows.call([minute - 60, minute])) }]
bingx << ['data_null', NUMBER_TF, bfrom, { k => ok({ 'code' => 0, 'data' => nil }) }]
number_cases(bx_row, minute * 1000).each { |name, r| bingx << ["row_#{name}", NUMBER_TF, bfrom, { k => ok(bx_body.call([r])) }] }

OTHER_VENUES = {
  'kucoin' => [Exchanges::Kucoin, 'KuCoin', kucoin, 'BTC-USDT', %w[app/models/exchanges/kucoin.rb]],
  'bitget' => [Exchanges::Bitget, 'Bitget', bitget, 'BTCUSDT', %w[app/models/exchanges/bitget.rb]],
  'bybit' => [Exchanges::Bybit, 'Bybit', bybit, 'BTCUSDT', %w[app/models/exchanges/bybit.rb]],
  'mexc' => [Exchanges::Mexc, 'MEXC', mexc, 'BTCUSDT', %w[app/models/exchanges/mexc.rb]],
  'bitrue' => [Exchanges::Bitrue, 'Bitrue', bitrue, 'BTCUSDT', %w[app/models/exchanges/bitrue.rb]],
  'bitvavo' => [Exchanges::Bitvavo, 'Bitvavo', bitvavo, 'BTC-EUR', %w[app/models/exchanges/bitvavo.rb]],
  'gemini' => [Exchanges::Gemini, 'Gemini', gemini, 'btcusd', %w[app/models/exchanges/gemini.rb]],
  'hyperliquid' => [Exchanges::Hyperliquid, 'Hyperliquid', hyperliquid, HL_COIN, %w[app/models/exchanges/hyperliquid.rb]],
  'coinbase' => [Exchanges::Coinbase, 'Coinbase', coinbase, 'BTC-USD', %w[app/models/exchanges/coinbase.rb]],
  'bingx' => [Exchanges::Bingx, 'BingX', bingx, 'BTC-USDT', %w[app/models/exchanges/bingx.rb]]
}.freeze

stub = ->(t) { TickerStub.new(t['ticker'], t['base'], AssetStub.new(t['crypto'] ? 'Cryptocurrency' : 'Stock')) }
cases_by_venue = { 'kraken' => [Exchanges::Kraken.new(name: 'Kraken'), kraken, TickerStub.new(KR_PAIR)],
                   'binance' => [Exchanges::Binance.new(name: 'Binance'), binance, TickerStub.new(BN_SYMBOL)],
                   'binance_us' => [Exchanges::BinanceUs.new(name: 'Binance.US'), binance_us, TickerStub.new(BN_SYMBOL)],
                   'alpaca' => [Exchanges::Alpaca.new(name: 'Alpaca'), alpaca, nil] }
sources = { 'kraken' => %w[app/models/exchanges/kraken.rb], 'binance' => %w[app/models/exchanges/binance.rb],
            'binance_us' => %w[app/models/exchanges/binance.rb app/models/exchanges/binance_us.rb],
            'alpaca' => %w[app/models/exchanges/alpaca.rb app/models/clients/alpaca.rb app/models/client.rb] }
OTHER_VENUES.each do |venue, (klass, name, list, pair, files)|
  cases_by_venue[venue] = [klass.new(name:), list, TickerStub.new(pair)]
  sources[venue] = files
end

venue = ARGV.fetch(0)
exchange, list, ticker = cases_by_venue.fetch(venue)
extend ActiveSupport::Testing::TimeHelpers
cases = list.map do |name, tf, start_at, http, case_ticker|
  Rails.cache.clear
  ScriptedCandles.http = http.transform_values { |replies| replies.map(&:dup) }
  ScriptedCandles.requests = []
  answer = begin
    result = travel_to(NOW) do
      exchange.get_indicator_candles(ticker: case_ticker ? stub.call(case_ticker) : ticker, start_at:, timeframe: TIMEFRAMES.fetch(tf))
    end
    if result.success?
      { 'candles' => result.data.map { |c| [c[0].utc.iso8601(6), *c[1..].map { |v| v.is_a?(BigDecimal) ? v.to_s('F') : "#{v.class}:#{v}" }] } }
    else
      { 'failure' => result.errors }
    end
  rescue StandardError => e
    { 'raised' => e.class.name, 'message' => e.message }
  end
  { 'venue' => venue, 'name' => name, 'timeframe' => tf, 'start_at' => start_at.utc.iso8601(6), 'now' => NOW.iso8601(6),
    'ticker' => case_ticker, 'http' => http, 'requests' => ScriptedCandles.requests }.compact.merge(answer)
ensure
  ScriptedCandles.http = nil
end

out = {
  'sources' => (sources.fetch(venue) + %w[app/models/exchange.rb app/models/exchange/candle_builder.rb app/models/utilities/hash.rb
                                          script/rust/record_candles.rb]).to_h { |f| [f, Digest::SHA256.file(Rails.root.join(f)).hexdigest] },
  'gems' => { 'honeymaker' => Honeymaker::VERSION },
  'cases' => cases
}
File.write(ARGV.fetch(1), "#{JSON.pretty_generate(out)}\n")
puts "wrote #{cases.size} cases to #{ARGV.fetch(1)}"
