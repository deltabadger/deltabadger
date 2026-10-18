# Records Exchange#get_tickers_prices(symbols:) on Kraken, Binance and KuCoin for rust/tests/tickers_prices.rs: what
# Rails' hash holds (each price as to_s('F')), its Failure's errors, or what it raised, and every request it made, in order.
#   bin/rails runner script/rust/record_tickers_prices.rb rust/tests/fixtures/tickers_prices_vectors.json
# Each case's bodies are served at the Faraday adapter beneath honeymaker, so its middleware, #with_rescue and the
# exchange's own parsing all run. Nothing leaves the machine. Re-run (script/rust/repin) when a file under "sources" changes.
require 'json'
require 'digest'

class Unscripted < Exception; end # rubocop:disable Lint/InheritException

# rubocop:disable Style/OneClassPerFile

Net::HTTP.prepend(Module.new { def connect = raise(Unscripted, "real connection to #{address}") })

module ScriptedPrices
  mattr_accessor :http, :requests

  NETWORK = {
    'pre_send' => -> { Faraday::ConnectionFailed.new(Errno::ECONNREFUSED.new('connect(2)')) },
    'post_send' => -> { Faraday::TimeoutError.new(Net::ReadTimeout.new) }
  }.freeze

  module Adapter
    def call(env)
      raise Unscripted, "no case is running: #{env.url}" if ScriptedPrices.http.nil?

      query = Rack::Utils.parse_query(env.url.query).map { |k, v| "#{k}=#{v}" }.sort.join('&')
      key = "#{env.method.to_s.upcase} #{env.url.path}#{"?#{query}" unless query.empty?}"
      ScriptedPrices.requests << key
      queue = ScriptedPrices.http[key] or raise Unscripted, "unscripted call #{key}"
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
Faraday::Adapter.lookup_middleware(:net_http_persistent).prepend(ScriptedPrices::Adapter)

def ok(body) = [{ 'status' => 200, 'body' => body }]
def status(code, body) = [{ 'status' => code, 'body' => body }]
def network(kind) = [{ 'network' => kind, 'message' => kind }]

# ---- Kraken: one bulk Ticker, then one Ticker?pair= per wanted code the bulk did not return as a key.
def kr(last) = { 'a' => ['1.0', '1', '1.000'], 'b' => ['1.0', '1', '1.000'], 'c' => last, 'v' => %w[1 1] }
def kr_body(result) = { 'error' => [], 'result' => result }
BULK = 'GET /0/public/Ticker'.freeze
def pair(code) = "GET /0/public/Ticker?pair=#{code}"

kraken = []
kraken << ['bulk_all', %w[ETHEUR SOLEUR],
           { BULK => ok(kr_body('ETHEUR' => kr(['2500.01', '0.1']), 'SOLEUR' => kr(['150.001', '1']), 'ADAEUR' => kr(['0.4', '1']))) }]
kraken << ['renamed_pair', %w[XBTEUR ETHEUR],
           { BULK => ok(kr_body('XXBTZEUR' => kr(['49995.3', '0.1']), 'ETHEUR' => kr(['2500.01', '0.1']))),
             pair('XBTEUR') => ok(kr_body('XXBTZEUR' => kr(['49995.3', '0.1']))) }]
kraken << ['fallback_in_order', %w[SOLEUR XBTEUR DOTEUR],
           { BULK => ok(kr_body('ADAEUR' => kr(['0.4', '1']))),
             pair('SOLEUR') => ok(kr_body('SOLEUR' => kr(['150.001', '1']))), pair('XBTEUR') => ok(kr_body('XXBTZEUR' => kr(['49995.3', '1']))),
             pair('DOTEUR') => ok(kr_body('DOTEUR' => kr(['4.1234', '1']))) }]
kraken << ['fallback_first_entry', %w[XBTEUR], { BULK => ok(kr_body({})),
                                                 pair('XBTEUR') => ok(kr_body('XXBTZEUR' => kr(['1.5', '1']), 'XBTEUR' => kr(['2.5', '1']))) }]
kraken << ['unknown_pair_skipped', %w[ETHEUR OLDEUR],
           { BULK => ok(kr_body('ETHEUR' => kr(['2500.01', '0.1']))), pair('OLDEUR') => ok({ 'error' => ['EQuery:Unknown asset pair'] }) }]
kraken << ['unknown_pair_among_errors', %w[ETHEUR OLDEUR],
           { BULK => ok(kr_body('ETHEUR' => kr(['2500.01', '0.1']))),
             pair('OLDEUR') => ok({ 'error' => ['EGeneral:Temporary lockout', 'EQuery:Unknown asset pair'] }) }]
kraken << ['fallback_error_fails_all', %w[ETHEUR OLDEUR SOLEUR],
           { BULK => ok(kr_body('ETHEUR' => kr(['2500.01', '0.1']))), pair('OLDEUR') => ok({ 'error' => ['EGeneral:Internal error'] }),
             pair('SOLEUR') => ok(kr_body('SOLEUR' => kr(['150.001', '1']))) }]
kraken << ['fallback_empty_result', %w[OLDEUR], { BULK => ok(kr_body({})), pair('OLDEUR') => ok(kr_body({})) }]
kraken << ['fallback_missing_result', %w[OLDEUR], { BULK => ok(kr_body({})), pair('OLDEUR') => ok({ 'error' => [] }) }]
kraken << ['fallback_missing_error', %w[OLDEUR], { BULK => ok(kr_body({})), pair('OLDEUR') => ok({ 'result' => {} }) }]
kraken << ['fallback_unreadable_price', %w[OLDEUR], { BULK => ok(kr_body({})), pair('OLDEUR') => ok(kr_body('OLDEUR' => kr(%w[x 1]))) }]
kraken << ['fallback_missing_c', %w[OLDEUR], { BULK => ok(kr_body({})), pair('OLDEUR') => ok(kr_body('OLDEUR' => { 'a' => %w[1 1 1] })) }]
kraken << ['fallback_transport', %w[OLDEUR], { BULK => ok(kr_body({})), pair('OLDEUR') => network('pre_send') }]
kraken << ['fallback_timeout', %w[OLDEUR], { BULK => ok(kr_body({})), pair('OLDEUR') => network('post_send') }]
kraken << ['fallback_http_500', %w[OLDEUR], { BULK => ok(kr_body({})), pair('OLDEUR') => status(500, 'upstream failed') }]
kraken << ['bulk_error', %w[ETHEUR], { BULK => ok({ 'error' => ['EService:Unavailable'] }) }]
kraken << ['bulk_two_errors', %w[ETHEUR], { BULK => ok({ 'error' => ['EService:Unavailable', 'EGeneral:Internal error'] }) }]
kraken << ['bulk_missing_error', %w[ETHEUR], { BULK => ok({ 'result' => {} }) }]
kraken << ['bulk_missing_result', %w[ETHEUR], { BULK => ok({ 'error' => [] }) }]
kraken << ['bulk_null_result', %w[ETHEUR], { BULK => ok({ 'error' => [], 'result' => nil }) }]
kraken << ['bulk_transport', %w[ETHEUR], { BULK => network('pre_send') }]
kraken << ['bulk_timeout', %w[ETHEUR], { BULK => network('post_send') }]
kraken << ['bulk_http_500', %w[ETHEUR], { BULK => status(500, 'upstream failed') }]
kraken << ['bulk_http_400_json', %w[ETHEUR], { BULK => status(400, { 'error' => ['EGeneral:Invalid arguments'] }) }]
kraken << ['bulk_not_json', %w[ETHEUR], { BULK => ok('<html>') }]
kraken << ['unwanted_unreadable_ignored', %w[ETHEUR], { BULK => ok(kr_body('ETHEUR' => kr(['2500.01', '1']), 'ADAEUR' => { 'a' => [] })) }]
# A returned key whose price is no finite decimal: omitted, and never asked again.
{ 'c_null' => [nil, '1'], 'c_empty' => [], 'c_word' => %w[x 1], 'c_infinity' => %w[Infinity 1], 'c_minus_infinity' => ['-Infinity'],
  'c_nan' => %w[NaN 1], 'c_blank' => ['', '1'], 'c_list' => [['1'], '1'], 'c_string' => 'x', 'c_string_digit' => '7',
  'c_hash' => {} }.each do |name, c|
  kraken << ["returned_#{name}", %w[ETHEUR SOLEUR], { BULK => ok(kr_body('ETHEUR' => kr(c), 'SOLEUR' => kr(['150.001', '1']))) }]
end
kraken << ['returned_c_missing', %w[ETHEUR], { BULK => ok(kr_body('ETHEUR' => { 'a' => %w[1 1 1] })) }]
kraken << ['returned_c_null_field', %w[ETHEUR], { BULK => ok(kr_body('ETHEUR' => kr(nil))) }]
kraken << ['returned_c_true', %w[ETHEUR], { BULK => ok(kr_body('ETHEUR' => kr(true))) }]
kraken << ['returned_entry_string', %w[ETHEUR], { BULK => ok(kr_body('ETHEUR' => 'x')) }]
# The digits Rails keeps: BigDecimal(value.to_s), a JSON number by its written form.
{ 'int' => [64_321], 'big_int' => [9_007_199_254_740_993], 'float17' => [1.2345678901234567], 'string17' => ['1.2345678901234567'],
  'zero' => ['0'], 'negative' => ['-5.5'], 'exponent' => ['1e5'], 'small_float' => [1e-07], 'padded' => [' 7.5 '], 'underscore' => ['1_000'],
  'hex' => ['0x10'], 'trailing_dot' => ['5.'], 'leading_dot' => ['.5'], 'plus' => ['+3'] }.each do |name, c|
  kraken << ["number_#{name}", %w[ETHEUR], { BULK => ok(kr_body('ETHEUR' => kr(c))) }]
end

# ---- Binance: /api/v3/ticker/price, every symbol.
PRICE = 'GET /api/v3/ticker/price'.freeze
def bn(rows) = { PRICE => ok(rows) }
def row(symbol, price) = { 'symbol' => symbol, 'price' => price }
binance = []
binance << ['strings', %w[BTCUSDT], bn([row('BTCUSDT', '64000.05000000'), row('ETHUSDT', '2500.47000000'), row('DEADUSDT', '0.00000000')])]
{ 'int' => 64_321, 'big_int' => 9_007_199_254_740_993, 'float17' => 1.2345678901234567, 'string17' => '1.2345678901234567', 'small_float' => 1e-07,
  'zero_int' => 0, 'exponent' => '1e3', 'padded' => ' 5 ', 'negative' => '-2.5' }.each do |name, price|
  binance << ["number_#{name}", %w[BTCUSDT], bn([row('BTCUSDT', price), row('ETHUSDT', '2500.47')])]
end
# String#to_d's lenient reading: the listed divergence (Q6) for those Rust cannot read as a decimal.
{ 'abc' => 'abc', '12x' => '12x', 'blank' => '', 'infinity' => 'Infinity', 'nan' => 'NaN', 'underscore' => '1_000' }.each do |name, price|
  binance << ["string_#{name}", %w[BTCUSDT], bn([row('BTCUSDT', price), row('ETHUSDT', '2500.47')])]
end
binance << ['duplicate_symbol', %w[BTCUSDT], bn([row('BTCUSDT', '1.5'), row('BTCUSDT', '2.5')])]
binance << ['missing_price', %w[BTCUSDT], bn([{ 'symbol' => 'BTCUSDT' }])]
binance << ['null_price', %w[BTCUSDT], bn([row('BTCUSDT', nil)])]
binance << ['true_price', %w[BTCUSDT], bn([row('BTCUSDT', true)])]
binance << ['list_price', %w[BTCUSDT], bn([row('BTCUSDT', ['1'])])]
binance << ['missing_symbol', %w[BTCUSDT], bn([{ 'price' => '1' }])]
binance << ['row_string', %w[BTCUSDT], bn(['BTCUSDT'])]
binance << ['object_body', %w[BTCUSDT], bn({ 'symbol' => 'BTCUSDT', 'price' => '1' })]
binance << ['empty', %w[BTCUSDT], bn([])]
binance << ['http_400', %w[BTCUSDT], { PRICE => status(400, { 'code' => -1121, 'msg' => 'Invalid symbol.' }) }]
binance << ['http_500_empty', %w[BTCUSDT], { PRICE => status(500, '') }]
binance << ['http_503_text', %w[BTCUSDT], { PRICE => status(503, 'Service Unavailable') }]
binance << ['transport', %w[BTCUSDT], { PRICE => network('pre_send') }]
binance << ['timeout', %w[BTCUSDT], { PRICE => network('post_send') }]
binance << ['not_json', %w[BTCUSDT], { PRICE => ok('<html>') }]

# ---- KuCoin: allTickers, kept when positive.
ALL = 'GET /api/v1/market/allTickers'.freeze
def kc(rows) = { ALL => ok({ 'code' => '200000', 'data' => { 'time' => 1_788_000_000_000, 'ticker' => rows } }) }
def last(symbol, price) = { 'symbol' => symbol, 'symbolName' => symbol, 'last' => price }
kucoin = []
kucoin << ['strings', %w[BTC-USDT], kc([last('BTC-USDT', '64000.1'), last('ETH-USDT', '2500.47'), last('DEAD-USDT', '0'), last('NEW-USDT', nil),
                                        { 'symbol' => 'BARE-USDT' }, last('NEG-USDT', '-1')])]
{ 'int' => 64_321, 'big_int' => 9_007_199_254_740_993, 'float17' => 1.2345678901234567, 'string17' => '1.2345678901234567', 'small_float' => 1e-07,
  'exponent' => '1e3', 'padded' => ' 5 ' }.each do |name, price|
  kucoin << ["number_#{name}", %w[BTC-USDT], kc([last('BTC-USDT', price), last('ETH-USDT', '2500.47')])]
end
{ 'abc' => 'abc', '12x' => '12x', 'blank' => '', 'infinity' => 'Infinity', 'nan' => 'NaN' }.each do |name, price|
  kucoin << ["string_#{name}", %w[BTC-USDT], kc([last('BTC-USDT', price), last('ETH-USDT', '2500.47')])]
end
kucoin << ['duplicate_symbol', %w[BTC-USDT], kc([last('BTC-USDT', '1.5'), last('BTC-USDT', '0'), last('ETH-USDT', '2'), last('ETH-USDT', '3')])]
kucoin << ['missing_symbol', %w[BTC-USDT], kc([{ 'last' => '1' }])]
kucoin << ['true_last', %w[BTC-USDT], kc([last('BTC-USDT', true)])]
kucoin << ['hash_last', %w[BTC-USDT], kc([last('BTC-USDT', {})])]
kucoin << ['row_string', %w[BTC-USDT], kc(['BTC-USDT'])]
kucoin << ['ticker_hash', %w[BTC-USDT], kc({ 'symbol' => 'BTC-USDT' })]
kucoin << ['empty', %w[BTC-USDT], kc([])]
kucoin << ['no_data', %w[BTC-USDT], { ALL => ok({ 'code' => '400100', 'msg' => 'Too many requests' }) }]
kucoin << ['null_ticker', %w[BTC-USDT], { ALL => ok({ 'code' => '200000', 'data' => { 'time' => 1 } }) }]
kucoin << ['data_string', %w[BTC-USDT], { ALL => ok({ 'code' => '200000', 'data' => 'x' }) }]
kucoin << ['http_500', %w[BTC-USDT], { ALL => status(500, { 'code' => '500000', 'msg' => 'Internal error' }) }]
kucoin << ['transport', %w[BTC-USDT], { ALL => network('pre_send') }]
kucoin << ['timeout', %w[BTC-USDT], { ALL => network('post_send') }]
kucoin << ['not_json', %w[BTC-USDT], { ALL => ok('<html>') }]

exchanges = { 'kraken' => Exchanges::Kraken.new(name: 'Kraken'), 'binance' => Exchanges::Binance.new(name: 'Binance'),
              'kucoin' => Exchanges::Kucoin.new(name: 'KuCoin') }
cases = { 'kraken' => kraken, 'binance' => binance, 'kucoin' => kucoin }.flat_map do |venue, list|
  list.map do |name, symbols, http|
    Rails.cache.clear
    ScriptedPrices.http = http.transform_values { |replies| replies.map(&:dup) }
    ScriptedPrices.requests = []
    answer = begin
      result = exchanges.fetch(venue).get_tickers_prices(symbols:, force: true)
      if result.success?
        { 'prices' => result.data.transform_values { |p| p.to_d.to_s('F') } }
      else
        { 'failure' => result.errors.map(&:to_s) }
      end
    rescue StandardError => e
      # A KeyError is Utilities::Hash.dig_or_raise's, whose text the failed tick reports.
      { 'raised' => e.class.name, 'message' => (e.message if e.is_a?(KeyError)) }.compact
    end
    { 'venue' => venue, 'name' => name, 'symbols' => symbols, 'http' => http, 'requests' => ScriptedPrices.requests }.merge(answer)
  ensure
    ScriptedPrices.http = nil
  end
end

out = {
  'sources' => %w[app/models/exchanges/kraken.rb app/models/exchanges/binance.rb app/models/exchanges/kucoin.rb app/models/utilities/hash.rb
                  script/rust/record_tickers_prices.rb].to_h { |f| [f, Digest::SHA256.file(Rails.root.join(f)).hexdigest] },
  'gems' => { 'honeymaker' => Honeymaker::VERSION },
  'cases' => cases
}
File.write(ARGV.fetch(0), "#{JSON.pretty_generate(out)}\n")
puts "wrote #{cases.size} cases to #{ARGV.fetch(0)}"
