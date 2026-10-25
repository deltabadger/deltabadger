# Records Exchange#get_indicator_candles (the candle read the conditions use) for rust/tests/candles.rs: the candles
# Rails returns (time ISO8601, then open, high, low, close and volume as to_s('F')), its Failure's errors, or what it
# raised, and every request it made, in order. Every case runs at NOW (travel_to): Binance's walk ends against
# `timeframe.ago`.
#   bin/rails runner script/rust/record_candles.rb kraken rust/tests/fixtures/kraken_candles.json
#   (binance, binance_us and alpaca likewise)
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
      ScriptedCandles.requests << key
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

stub = ->(t) { TickerStub.new(t['ticker'], t['base'], AssetStub.new(t['crypto'] ? 'Cryptocurrency' : 'Stock')) }
cases_by_venue = { 'kraken' => [Exchanges::Kraken.new(name: 'Kraken'), kraken, TickerStub.new(KR_PAIR)],
                   'binance' => [Exchanges::Binance.new(name: 'Binance'), binance, TickerStub.new(BN_SYMBOL)],
                   'binance_us' => [Exchanges::BinanceUs.new(name: 'Binance.US'), binance_us, TickerStub.new(BN_SYMBOL)],
                   'alpaca' => [Exchanges::Alpaca.new(name: 'Alpaca'), alpaca, nil] }
sources = { 'kraken' => %w[app/models/exchanges/kraken.rb], 'binance' => %w[app/models/exchanges/binance.rb],
            'binance_us' => %w[app/models/exchanges/binance.rb app/models/exchanges/binance_us.rb],
            'alpaca' => %w[app/models/exchanges/alpaca.rb app/models/clients/alpaca.rb app/models/client.rb] }

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
