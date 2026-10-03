# Records what rust/src/sync must reproduce of Rails' pure mappings, from the app's own code.
#   bin/rails runner script/rust/record_sync_vectors.rb rust/tests/fixtures/sync_vectors.json
# Inputs are fixed and non-secret because the output is committed. Re-run when one of the files under
# "ported_sources" changes (rust/tests/sync_vectors.rs fails until then) and re-check the port against the change.
require 'json'
require 'digest'

rng = Random.new(20_261_002)
bits = ->(float) { [float].pack('G').unpack1('H*') }
quoted = ->(time) { time && ActiveRecord::Base.connection.quoted_date(AccountTransaction.type_for_attribute(:transacted_at).serialize(time)) }
vectors = {}

# ---- ApiKey#scrub and the 200-character cut of #record_sync_error! ----
key = { key: 'PKPARITY7KEY4TEST2ID', secret: 'parity-secret-Zq8fW3nL5xT1vB7mK2dR9hY4', passphrase: 'paper' }
texts = [
  'unauthorized.', '', 'Faraday::ConnectionFailed: Connection refused - connect(2) for "paper-api.alpaca.markets" port 443',
  'key PKPARITY7KEY4TEST2ID secret parity-secret-Zq8fW3nL5xT1vB7mK2dR9hY4 mode paper, newspaper, PAPER',
  'mail owner@example.com, Owner.Name+tag@sub.example.co.uk1 x@y.z a@b.io. @nolocal.com trailing@ a@b@c.com a@-.cc user@host.c0m user@host.com2',
  'see https://paper-api.alpaca.markets/v2/account?foo=1&bar=2 and http://h/p?x then https://no-query.example/path http://? http://a?b?c https://a.b/?',
  'xhttps://x.y/z?q=1,http://second/?a b https://third/x y?z',
  'token 0123456789abcdefghij short 0123456789abcdefghi letters abcdefghijklmnopqrstuvwxyz mixed abcdefghijklmnopqrs_-9 dashed -------------------1',
  'digits 12345678 123456789 1234567890123 a123456789b 12-345678901',
  "unicode zażółć gęślą jaźń #{'é' * 210}",
  "long #{'e' * 190} owner@example.com tail #{'z' * 40}",
  'order 9f8e7d6c-5b4a-3928-1706-f5e4d3c2b1a0 id 9f8e7d6c5b4a39281706f5e4d3c2b1a0',
  "tabs\thttps://x/y?z=1\tnext\nline http://x/y?z"
]
keys = [key, { key: 'k', secret: 's', passphrase: nil }, { key: 'abc', secret: 'abcdef', passphrase: 'abc' }, { key: '', secret: ' ', passphrase: 'live' }]
vectors['scrub'] = keys.flat_map do |k|
  api_key = ApiKey.new(**k)
  texts.map { |text| { 'key' => k.transform_keys(&:to_s), 'text' => text, 'scrubbed' => api_key.scrub(text), 'stored' => api_key.scrub(text)[0, ApiKey::SYNC_ERROR_LIMIT] } }
end

# ---- Exchanges::Alpaca.split_ratio_label, and the Float#rationalize beneath it ----
pairs = [%w[1 10], %w[10 1], %w[2 3], %w[3 2], %w[1 50], %w[1 100], %w[100 2], %w[1000 1001], %w[5 5], %w[0 5], %w[5 0], %w[-1 5], %w[3.3333 33.333],
         %w[3.333 4.9995], %w[7 1], %w[1 7], %w[0.000001 0.00002], %w[123456789 987654321], %w[10 30], %w[1 1.0005], %w[1 1.002], %w[4 1]] +
        Array.new(300) do
          old = (rng.rand * (10**rng.rand(0..4))).round(rng.rand(0..6)) + 0.001
          factor = [2, 3, 4, 5, 7, 10, 20, 50, 1.5, 2.5, 0.5, 0.1, 0.2, 0.25, 1.0 / 3, rng.rand * 10 + 0.01].sample(random: rng)
          [old.to_d.to_s('F'), (old.to_d * factor.to_d).round(rng.rand(0..9)).to_d.to_s('F')]
        end
vectors['split_ratio_label'] = pairs.map { |old, new| [old, new, Exchanges::Alpaca.split_ratio_label(old, new)] }
vectors['rationalize'] = Array.new(300) do
  f = rng.rand * (10**rng.rand(-3..3)) + 1.0e-6
  eps = f * [0.001, 0.0001, 0.01, 0.1].sample(random: rng)
  r = f.rationalize(eps)
  [bits.(f), bits.(eps), r.numerator.to_s, r.denominator.to_s]
end

# ---- Exchanges::Alpaca#normalize_activity and #merge_split_entries, with a crypto pair index of two listings ----
alpaca = Exchanges::Alpaca.new
pair = ->(base, quote, symbol) { Ticker.new(base:, quote:, base_asset: Asset.new(symbol:, category: 'Cryptocurrency')) }
alpaca.instance_variable_set(:@crypto_position_index, { 'ETHUSD' => pair.('ETH', 'USD', 'ETH'), 'XBTUSDT' => pair.('XBT', 'USDT', 'BTC'), 'NOSYMUSD' => pair.('NOSYM', 'USD', nil) })
nta = ->(id, type, **fields) { { 'id' => id, 'activity_type' => type }.merge(fields.transform_keys(&:to_s)) }
fill = ->(id, symbol, side, qty, price, time) { { 'id' => id, 'activity_type' => 'FILL', 'symbol' => symbol, 'side' => side, 'qty' => qty, 'price' => price, 'transaction_time' => time, 'order_id' => "o-#{id}" } }
streams = [
  [fill.('f1', 'AAPL', 'buy', '0.359712230', '166.80', '2026-09-10T14:30:00.123456789Z'), fill.('f2', 'AAPL', 'sell', '5', '155.00', '2026-03-20T15:00:00Z'),
   fill.('f3', 'BTC/USD', 'buy', '0.001130165', '64149.97', '2026-09-12T03:04:05.5-04:00'), fill.('f4', 'ETHUSD', 'sell', '0.5', '2500.1', '2026-09-12T10:00:00+00:00'),
   fill.('f5', 'XBTUSDT', 'buy', '1', '2', '2026-09-12T10:00:00.999999999Z'), fill.('f6', 'XYZUSD', 'buy', 3, 1.5, '2026-09-12T11:00:00Z'),
   fill.('f7', '/USD', 'buy', '1', '1', '2026-09-12T11:00:00Z'), fill.('f8', 'A/B/C', 'sell_short', '1e-9', '1.0e3', '2026-09-12T11:00:00Z'),
   fill.('f9', nil, 'buy', nil, nil, '2026-09-12T11:00:00Z'), fill.('', 'AAPL', 'buy', '-1', '0.1', '2026-12-31T23:59:59.999999Z')],
  [nta.('csd', 'CSD', net_amount: '5000.00', date: '2026-03-19', group_id: 'g'), nta.('csw', 'CSW', net_amount: '-1200.50', date: '2026-03-19'),
   nta.('j1', 'JNLC', net_amount: '25'), nta.('j2', 'JNLC', net_amount: '-30.10', transaction_time: '2026-09-06T12:13:14.000001Z'), nta.('j3', 'OCT', net_amount: nil, date: '2026-01-01'),
   nta.('j4', 'ACATC', net_amount: '-0', date: '2026-02-28'), nta.('j5', 'JNLC', net_amount: 12.5, date: '2028-02-29'), nta.('j6', 'CSD', net_amount: '1', date: '2026-03-19', status: 'canceled')],
  (Exchanges::Alpaca.const_get(:DIVIDEND_INCOME_TYPES) + Exchanges::Alpaca.const_get(:WITHHOLDING_TYPES) + Exchanges::Alpaca.const_get(:CASH_FEE_TYPES) + %w[INT PTR]).flat_map do |type|
    [nta.("#{type}-sym", type, net_amount: '-1.23', symbol: 'AAPL', date: '2026-09-01', group_id: 'pay'), nta.("#{type}-bare", type, net_amount: '0.07', date: '2026-09-02')]
  end,
  [nta.('roc1', 'DIVROC', net_amount: '7.25', symbol: 'QQQM', qty: '2', date: '2026-09-07'), nta.('roc2', 'DIVROC', net_amount: '-3.5', symbol: 'QQQM', date: '2026-09-08'),
   nta.('roc3', 'DIVROC', date: '2026-09-08'), nta.('cfee1', 'CFEE', net_amount: '0', symbol: 'ETHUSD', qty: '-0.000195', date: '2026-03-18'),
   nta.('cfee2', 'CFEE', symbol: 'UNKNOWNUSD', qty: '-0.5', date: '2026-03-18'), nta.('cfee3', 'CFEE', symbol: 'XBTUSDT', qty: '0.25', date: '2026-03-18'),
   nta.('cfee4', 'CFEE', symbol: 'NOSYMUSD', qty: '-1', date: '2026-03-18'), nta.('cfee5', 'CFEE', qty: '-1', date: '2026-03-18'),
   nta.('ma', 'MA', symbol: 'KLAC', qty: '-3', net_amount: '2100.75', date: '2026-09-01'), nta.('reo', 'REO', date: '2026-09-02'), nta.('odd', nil, qty: 2, net_amount: 1.0e-05, date: '2026-09-03'),
   nta.('opt', 'OPEXP', symbol: 'AAPL260918C00200000', qty: '-1', net_amount: '0', date: '2026-09-18', description: 'expired')],
  [nta.('s1', 'SPLIT', symbol: 'ZZTOP', qty: '-10', net_amount: '0', date: '2026-05-01'), nta.('s2', 'SPLIT', symbol: 'ZZTOP', qty: '30', net_amount: '0', date: '2026-05-01'),
   nta.('r1', 'SPLIT', symbol: 'QQTEST', qty: '-30', date: '2026-05-02'), nta.('r2', 'SPLIT', symbol: 'QQTEST', qty: '10', date: '2026-05-02'),
   nta.('t1', 'SSP', symbol: 'KLAC', qty: '-10', date: '2026-05-03'), nta.('t2', 'SSP', symbol: 'KLAC', qty: '15', date: '2026-05-03'), nta.('t3', 'SPLIT', symbol: 'KLAC', qty: '15', date: '2026-05-03'),
   nta.('u1', 'SPLIT', symbol: 'AAPL', qty: '-1', date: '2026-05-04'), nta.('u2', 'SPLIT', symbol: 'AAPL', qty: '4', date: '2026-05-05'),
   nta.('v1', 'SPLIT', symbol: 'AAA', qty: '-2', date: '2026-05-06'), nta.('v2', 'SPLIT', symbol: 'BBB', qty: '4', date: '2026-05-06'),
   nta.('w1', 'SPLIT', symbol: 'CCC', qty: '-1000', date: '2026-05-07'), nta.('w2', 'SPLIT', symbol: 'CCC', qty: '1001', date: '2026-05-07'),
   nta.('x1', 'SPLIT', symbol: 'DDD', qty: '5', date: '2026-05-08'), nta.('x2', 'SPLIT', symbol: 'DDD', qty: '5', date: '2026-05-08'),
   nta.('y1', 'SPLIT', qty: '-3.333', transaction_time: '2026-05-09T10:00:00Z'), nta.('y2', 'SPLIT', qty: '4.9995', transaction_time: '2026-05-09T11:00:00Z'),
   nta.('z1', 'SPLIT', symbol: 'EEE', qty: '-4', date: '2026-05-10', status: 'canceled'), nta.('z2', 'SPLIT', symbol: 'EEE', qty: '1', date: '2026-05-10')]
]
entry = lambda do |e|
  e.transform_keys(&:to_s).merge('entry_type' => AccountTransaction.entry_types.fetch(e[:entry_type].to_s), 'base_amount' => e[:base_amount].to_s('F'),
                                 'quote_amount' => e[:quote_amount]&.to_s('F'), 'fee_amount' => e[:fee_amount]&.to_s('F'), 'transacted_at' => quoted.(e[:transacted_at]),
                                 'raw' => e[:raw_data], 'raw_text' => AccountTransaction.new(raw_data: e[:raw_data]).instance_variable_get(:@attributes)['raw_data'].value_for_database).except('raw_data')
end
vectors['ledger_entries'] = streams.map do |activities|
  normalized = activities.filter_map { |a| alpaca.send(:normalize_activity, a.deep_dup) }
  { 'activities' => activities, 'entries' => alpaca.send(:merge_split_entries, normalized).map(&entry) }
end

# ---- ActiveModel::Type::Decimal as account_balances casts what the balance sync assigns, and Float#round beneath it ----
price = AccountBalance.type_for_attribute(:usd_price) # decimal(20,8)
free = AccountBalance.type_for_attribute(:free)       # decimal(32,16)
floats = [64_321.123456789, 0.1 + 0.2, 123_456_789.12345678, 1.0e-9, 5.0e-9, 2.675, 1.0000000049999999, 1.000000005, 0.000000015, 2500.123456785, 201.379999,
          99_999_999.999999995, 12_345_678.001953125, 1.0e15, 1.0e-20, 0.0, 227.52, 1.0 / 3, 2.0 / 3, 1.0e8 + 0.000000005, 4.35, 0.285, 1.005] +
         Array.new(400) { (rng.rand * (10**rng.rand(-9..9))).round(rng.rand(0..17)) } +
         Array.new(400) { (rng.rand(1_000_000_000) + 0.5 + ((rng.rand - 0.5) * 1.0e-6)) / 1.0e8 * (10**rng.rand(0..4)) } # at and around a half in the 8th place
vectors['float_round_8'] = floats.map { |f| [bits.(f), bits.(f.round(8))] }
vectors['cast_float_8'] = floats.map { |f| c = price.cast(f); [bits.(f), c.to_s('F'), bits.(c.to_f)] }
decimals = %w[1 0.125 2.5 -2.5 0.000000005 0.000000004999 123456789012345.123456785 1234567890123456789.123456785 1.23456789012345678951234567890123456 72.69348249 0
              0.12345678901234565 99123.45 -0.000000015 15241496599604.248123456789] +
           Array.new(300) { "#{rng.rand(10**rng.rand(1..12))}.#{Array.new(rng.rand(1..22)) { rng.rand(10) }.join}" }
vectors['cast_decimal'] = decimals.flat_map do |d|
  [[d, 8, price.cast(BigDecimal(d))], [d, 16, free.cast(BigDecimal(d))]].map { |text, scale, c| [text, scale, c.to_s('F'), bits.(c.to_f)] }
end

# ---- MarketData.get_prices' reading of one price ("h[id] = price.to_f if price") ----
vectors['price_to_f'] = [64_321.123456789, 2500, 0, '1.5', ' 2 ', '1e3', '0.30000000000000004', nil, false].map { |v| [v, v ? bits.(v.to_f) : nil] }

# ---- an answer as Rails holds it: `JSON.parse` as the app has it (what Faraday's :json middleware runs), re-generated ----
# This script runs inside the app (bin/rails runner), where Oj has replaced JSON.parse: a key met twice keeps its first
# place and its last value, at every level; more than 100 levels raise.
deep = ->(n) { ('[' * n) + (']' * n) }
vectors['json_canonical'] = [
  '{"x":1,"x":2,"y":3}', '{"a":{"x":1,"y":0,"x":2}}', '{"a":1,"b":2,"a":{"k":1,"k":[{"z":1,"z":null}]},"b":3,"a":4}', '{"a":1,"\u0061":2}', '{"k\"ey":1,"k\"ey":"two"}',
  '[{"id":"a","extra":{"x":1,"x":2}},{"id":"b"}]', '{}', '[]', 'null', '"text"', '7', ' { "a" : [ 1 , 2 ] , "b" : { } } ',
  '{"n":18446744073709551617,"m":-9223372036854775809,"f":1.0000000000000000001,"e":1e2,"z":-0.0,"s":"1e400"}',
  '{"s":"caf\u00e9 \ud83d\ude00 \n \" \\\\ <tag>","":0,"":1}', '{"a":[],"a":{},"a":[{"b":1,"b":2}]}',
  deep.(100), deep.(101), ('{"a":' * 100) + '1' + ('}' * 100), ('{"a":' * 101) + '1' + ('}' * 101), "[#{deep.(99)},#{deep.(99)}]", "[#{deep.(100)}]",
  '{"a":1,}', '[1,2', '{"a" 1}', '{a:1}', "{'a':1}", '[01]', '[1.]', '[NaN]', '[Infinity]', 'nul', '', '[1] x', '{"a":"\x"}'
].map do |text|
  begin
    [text, JSON.generate(JSON.parse(text)), nil]
  rescue JSON::NestingError
    [text, nil, 'nesting']
  rescue JSON::ParserError
    [text, nil, 'parser']
  rescue StandardError => e
    [text, nil, e.class.name]
  end
end

# ---- raw_data as the JSON column stores it: what AccountTransaction.new(raw_data: JSON.parse(text)) writes. In the app
# the parser and the encoder are Oj's (Oj.optimize_rails): a Float is written in sixteen significant digits (C's
# %0.16g; "%.1f" for a whole number; Float#to_s when the sixteen end in 0001 or 9999), an Integer exactly, a string
# with Rails' escapes. And the value is printed TWICE: assigning to a JSON attribute casts it through its own text
# (ActiveModel::Type::Helpers::Mutable#cast is deserialize(serialize(value))), and saving serialises what that read
# back. So a double whose sixteen digits round up past the largest double is stored as null. Each vector is a JSON
# text and the column text it becomes; the single serialisation is recorded beside it where the two differ.
once = ->(text) { AccountTransaction.type_for_attribute(:raw_data).serialize(JSON.parse(text)) }
stored = ->(text) { AccountTransaction.new(raw_data: JSON.parse(text)).instance_variable_get(:@attributes)['raw_data'].value_for_database }
float_tokens = %w[0.30000000000000004 1.2345678901234567 1.0 1e2 1E2 1e+2 -1e2 1e0 -0.0 0.0 -0 0 100 -7 1.5e3 1e15 1e16 1e17 1e19 -1e19 1e20 1e22 1e23 1e-4 1e-5 1e-7
                  0.0001 0.00001 0.000001 0.0000001 123456789012345.6 1234567890123456.7 12345678901234567.8 9223372036854775807 9223372036854775808
                  -9223372036854775808 -9223372036854775808.0 9223372036854774784.0 18446744073709551617 -123456789012345678901234567890 4.9e-324 5e-324
                  2.2250738585072014e-308 1.7976931348623157e308 1e308 1e-400 0.1 0.2 0.5 2.5 1.0000000000000000001 0.1234567890120001 5.0000000000099992
                  0.99999999999999989 123.456e-2 1.10 -1.5e-10 6.02214076e23 3.141592653589793 0.3333333333333333 0.6666666666666666 12345678901234567890.5
                  123e-20 9007199254740993 9007199254740993.0 4e15 4.5e15 123456789.125 1984.0207455399993 23692.989533599994 15.681457279499988
                  9671.23262804 3.3433394988700007 360.79250566700006 1.2345678900000012 12345678901.20001 0.7999999999999999 999999999999999.9
                  9999999999999999.0 0.00009999999999999999 0.0001234567890123456 1234567890123456e-20 -0.30000000000000004 1e-300 1.5e300]
float_tokens += Array.new(300) do
  digits = Array.new(rng.rand(1..19)) { rng.rand(10) }.join.sub(/\A0+(?=\d)/, '')
  point = rng.rand(0..digits.size)
  token = point.zero? ? "0.#{digits}" : "#{digits[0, point]}.#{digits[point..]}0"
  token += "e#{rng.rand(-25..25)}" if rng.rand(3).zero?
  rng.rand(6).zero? ? "-#{token}" : token
end
# Doubles whose sixteen digits end in 0001 or 9999: Oj prints those with Float#to_s.
# Seventeen digits high in a decade, where two doubles can share sixteen digits and the second printing can move.
float_tokens += Array.new(400) { "#{rng.rand(7..9)}.#{Array.new(16) { rng.rand(10) }.join}#{rng.rand(3).zero? ? "e#{rng.rand(-20..20)}" : ''}" }
float_tokens += Array.new(40_000) { rng.rand * (10**rng.rand(-3..12)) }.select { |d| format('%0.16g', d).then { |g| g.size >= 17 && g.end_with?('0001', '9999') } }.first(25).map(&:to_s)
texts = float_tokens.map { |t| "[#{t}]" } + [
  '{"s":"a<b>&c \u00e9 \/ \u2028\u2029 \t\n\r\b\f \" \\\\ \u0000\u0001\u001f\u007f \ud83d\ude00"}', '{"k<\"\\\\\u00e9":1,"\u2028":2,"":3}',
  "[\"#{(0..0x7f).map { |c| format('\u%04x', c) }.join}\"]", '{"a":[],"b":{},"c":[[]],"d":null,"e":false,"f":"","g":[1.5,-2,{"h":1e-7}],"i":true}',
  ' { "n" : 18446744073709551617 , "f":1.0000000000000000001, "n": [ 1e2 , "ab" ] } ', '{"x":0.1,"x":{"y":1e2,"y":0.30000000000000004}}', '"text"', '7', '7.0', 'null'
]
vectors['json_stored'] = texts.map { |text| [text, stored.(text), (once.(text) unless once.(text) == stored.(text))] }

# ---- a value that is not JSON where a later value of the same key overwrites it: refused, at any depth ----
vectors['json_overwritten'] = %w[wat tru nul 1e 01 -01 1. 2. .5 +1 NaN Infinity -Infinity TRUE 0x10 1_0 1.0e "\u12" [1,] {"a"} 'a'].flat_map do |bad|
  ["{\"qty\":#{bad},\"qty\":\"10\"}", "{\"a\":{\"b\":[1,{\"c\":#{bad},\"c\":1}]}}", "[{\"k\":[#{bad}],\"k\":0}]"]
end.map do |text|
  begin
    [text, JSON.generate(JSON.parse(text)), nil]
  rescue JSON::ParserError
    [text, nil, 'parser']
  end
end

# ---- activity times ----
vectors['times'] = {
  'trade' => ['2026-09-10T14:30:00.123456789Z', '2026-09-12T03:04:05.5-04:00', '2026-03-20T15:00:00Z', '2026-12-31T23:59:59.999999999+05:30'].map { |s| [s, quoted.(Time.parse(s).utc)] },
  'non_trade' => ['2026-03-19', '2028-02-29', '2026-09-06T12:13:14.000001Z', '2026-09-06T12:13:14+02:00'].map { |s| [s, quoted.(Time.zone.parse(s).utc)] },
  'after' => [Time.utc(2026, 9, 10, 14, 30, 0, 123_456), Time.utc(2026, 1, 1)].map { |t| [quoted.(t), (t.in_time_zone - 25.hours).iso8601] }
}

# ---- constants the port copies ----
vectors['constants'] = {
  'entry_types' => AccountTransaction.entry_types,
  'crypto_coingecko_ids' => Exchanges::Alpaca::CRYPTO_COINGECKO_IDS,
  'crypto_quotes' => Exchanges::Alpaca.const_get(:CRYPTO_QUOTES),
  'cash_activity_types' => Exchanges::Alpaca.const_get(:CASH_ACTIVITY_TYPES),
  'split_types' => Exchanges::Alpaca.const_get(:SPLIT_TYPES),
  'fiat_currencies' => Tax::PriceService::FIAT_CURRENCIES,
  'cash_categories' => Fiat::CATEGORIES,
  'sync_error_limit' => ApiKey::SYNC_ERROR_LIMIT,
  'asset_catch_up_days' => AccountTransactionSync.const_get(:ASSET_CATCH_UP).in_days.to_i,
  'feed_retention_days' => BotActivityLog::PruneJob::RETENTION.in_days.to_i,
  'transfer_window_hours' => TransferMatcher::WINDOW.in_hours.to_i,
  'transfer_tolerance' => TransferMatcher::TOLERANCE.to_s('F'),
  'tombstone_prefix' => MarketData::TICKER_TOMBSTONE_PREFIX,
  'invalid_key_errors' => Exchanges::Alpaca::ERRORS.fetch(:invalid_key),
  'permission_errors' => Exchanges::Alpaca::ERRORS[:permission_denied].to_a,
  'recurring' => YAML.load_file(Rails.root.join('config/recurring.yml'), aliases: true).fetch('production')
                     .slice('sync_all_account_transactions_job', 'sync_all_account_balances_job').transform_values { |j| j.slice('class', 'schedule') }
}

vectors['ported_sources'] = %w[
  app/services/account_transaction_sync.rb app/services/account_balance/sync.rb app/services/transfer_matcher.rb
  app/jobs/account_transaction/sync_job.rb app/jobs/account_transaction/sync_all_job.rb app/jobs/account_balance/sync_job.rb
  app/jobs/account_balance/sync_all_job.rb app/models/api_key.rb app/models/concerns/api_key_failure_handling.rb app/models/exchanges/alpaca.rb
  app/models/clients/alpaca.rb app/models/ticker.rb app/models/account_transaction.rb config/initializers/oj.rb
].to_h { |file| [file, Digest::SHA256.file(Rails.root.join(file)).hexdigest] }
# Which parser read the JSON vectors. Inside the app `JSON.parse` is Oj's (config/initializers/oj.rb: Oj.optimize_rails):
# a key written twice keeps its last value. The json gem on its own (3.0) raises JSON::ParserError there, so the
# vectors hold only while the app parses with Oj: the two gem versions are pinned with them.
lock = File.read(Rails.root.join('Gemfile.lock'))
vectors['json_parser'] = {
  'json_parse_is_oj' => JSON.method(:parse).source_location.nil? && defined?(Oj) ? true : false,
  'gems' => %w[json oj].to_h { |gem| [gem, lock[/^    #{gem} \((.+)\)$/, 1]] }
}

File.write(ARGV.fetch(0), "#{JSON.pretty_generate(vectors)}\n")
puts "wrote #{ARGV.fetch(0)}"
