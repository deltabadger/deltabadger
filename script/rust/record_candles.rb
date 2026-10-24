# Records Exchange#get_indicator_candles (the candle read the conditions use) for rust/tests/candles.rs: the candles
# Rails returns (time ISO8601, then open, high, low, close and volume as to_s('F')), its Failure's errors, or what it
# raised, and every request it made, in order.
#   bin/rails runner script/rust/record_candles.rb kraken rust/tests/fixtures/kraken_candles.json
# Each case's bodies are served at the Faraday adapter beneath honeymaker, so its middleware, #with_rescue and the
# exchange's own parsing and CandleBuilder all run. Nothing leaves the machine. Re-run (script/rust/repin) when a file
# under "sources" changes. One venue per fixture; later venues add a section below.
require 'json'
require 'digest'

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
TickerStub = Struct.new(:ticker)

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

cases_by_venue = { 'kraken' => [Exchanges::Kraken.new(name: 'Kraken'), kraken, TickerStub.new(KR_PAIR)] }
sources = { 'kraken' => %w[app/models/exchanges/kraken.rb] }

venue = ARGV.fetch(0)
exchange, list, ticker = cases_by_venue.fetch(venue)
cases = list.map do |name, tf, start_at, http|
  Rails.cache.clear
  ScriptedCandles.http = http.transform_values { |replies| replies.map(&:dup) }
  ScriptedCandles.requests = []
  answer = begin
    result = exchange.get_indicator_candles(ticker:, start_at:, timeframe: TIMEFRAMES.fetch(tf))
    if result.success?
      { 'candles' => result.data.map { |c| [c[0].utc.iso8601(6), *c[1..].map { |v| v.is_a?(BigDecimal) ? v.to_s('F') : "#{v.class}:#{v}" }] } }
    else
      { 'failure' => result.errors }
    end
  rescue StandardError => e
    { 'raised' => e.class.name, 'message' => e.message }
  end
  { 'venue' => venue, 'name' => name, 'timeframe' => tf, 'start_at' => start_at.utc.iso8601(6), 'http' => http,
    'requests' => ScriptedCandles.requests }.merge(answer)
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
