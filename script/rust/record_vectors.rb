# Records what the Rust crate in rust/ must reproduce, from the app's own code.
#   bin/rails runner script/rust/record_vectors.rb rust/tests/fixtures/ruby_vectors.json
# Inputs are fixed and non-secret because the output is committed. Re-run when Rails, bcrypt or rotp
# move (rust/tests/codec.rs pins activerecord, bcrypt, bigdecimal and rotp) or when an enum changes.
require 'json'
require 'bcrypt'

SECRET = 'rust-fixture-secret-key-base'.freeze

# The key Rails derives from a primary key + salt, computed explicitly so no global encryption
# config is mutated. check_against_app! proves this path equals the app's configured one.
def key_provider(primary, salt)
  bytes = ActiveSupport::KeyGenerator.new(primary, hash_digest_class: OpenSSL::Digest::SHA256, iterations: 2**16)
                                     .generate_key(salt, 32)
  ActiveRecord::Encryption::KeyProvider.new(ActiveRecord::Encryption::Key.new(bytes))
end

def encrypt_with(provider, model, attribute, plain)
  ActiveRecord::Encryption.with_encryption_context(key_provider: provider) do
    model.type_for_attribute(attribute).serialize(plain)
  end
end

def check_against_app!
  config = ActiveRecord::Encryption.config
  provider = key_provider(Array(config.primary_key).first, config.key_derivation_salt)
  cipher = ApiKey.type_for_attribute(:secret).serialize('probe') # the app's real attribute path
  plain = ActiveRecord::Encryption.with_encryption_context(key_provider: provider) do
    ApiKey.type_for_attribute(:secret).deserialize(cipher)
  end
  raise 'explicit key derivation differs from the app configuration' unless plain == 'probe'
end

check_against_app!
primary = EncryptionKeys.derived_primary_key(SECRET)
salt = EncryptionKeys.derived_salt(SECRET)
provider = key_provider(primary, salt)
long = "-----BEGIN PRIVATE KEY-----\n#{'MIIEvQIBADANBgkqhkiG9w0BAQEFAASC' * 12}\n-----END PRIVATE KEY-----"
plains = {
  'short' => [ApiKey, :secret, 'kR4k3n-s3cr3t/+=='],
  'unicode' => [ApiKey, :key, 'zażółć 🦡'],
  'compressed' => [ApiKey, :rsa_signature_key, long],
  # Through the app's own path: ROTP returns a binary string, so Rails adds an "e" (encoding) header.
  'otp_seed' => [User, :otp_secret_key, User.new.tap(&:otp_regenerate_secret).otp_secret_key],
  'app_config' => [AppConfig, :value, '{"engine":"rust"}']
}
passwords = ['correct horse ☃', 'x' * 100] # the second is past bcrypt's 72-byte limit
totp_times = [0, 59, 1_790_000_000, 1_790_000_029, 1_790_000_030]
times = [Time.utc(2026, 9, 28, 22, 1, 3, 734_946), Time.utc(2026, 9, 28, 22, 1, 3)]
rng = Random.new(42)
floats = [30.0003, 0.1 + 0.2, 72_960.5, 0.00041118, 1.0e-08, 1.0e-18, 123_456_789.12345679, 1.0 / 3, 60.0, -2.5,
          99_999_999_999_999_999.0] + Array.new(300) { (rng.rand * (10**rng.rand(-12..12))).round(rng.rand(0..17)) }
decimal = Transaction.type_for_attribute(:amount)

vectors = {
  'secret_key_base' => SECRET,
  'primary_key' => primary,
  'key_derivation_salt' => salt,
  'ciphertexts' => plains.transform_values { |(model, attr, plain)| { 'plain' => plain, 'cipher' => encrypt_with(provider, model, attr, plain) } },
  'bcrypt' => passwords.map { |pw| { 'password' => pw, 'hash' => BCrypt::Password.create(pw, cost: 11).to_s } },
  'totp' => { 'seed' => 'JBSWY3DPEHPK3PXPJBSWY3DP', 'codes' => totp_times.map { |t| [t, ROTP::TOTP.new('JBSWY3DPEHPK3PXPJBSWY3DP').at(t)] } },
  'times' => times.map { |t| [t.iso8601(6), Transaction.connection.quoted_date(t)] },
  # IEEE bits, not JSON numbers: Ruby's JSON.generate prints 123456789.12345679 as 123456789.1234568.
  'decimals' => floats.map { |f| [[f].pack('G').unpack1('H*'), decimal.cast(f).to_s('F')] },
  # Floats outside rust_decimal's range (scale > 28, >= ~1e29): Ruby reads them fine, and so must BigDec.
  'decimals_wide' => [1.234567890123456e-14, 3.0e-20, 1.5e30, -2.5e-15, 1.0e-30, 123_456_789_012_345.6e20, 5.0e-324, 1.7976931348623157e308]
                       .map { |f| [[f].pack('G').unpack1('H*'), decimal.cast(f).to_s('F')] },
  'enums' => {
    'bot_status' => Bot.statuses, 'rule_status' => Rule.statuses, 'transaction_status' => Transaction.statuses, 'transaction_side' => Transaction.sides,
    'transaction_order_type' => Transaction.order_types, 'transaction_external_status' => Transaction.external_statuses,
    'api_key_status' => ApiKey.statuses, 'api_key_key_type' => ApiKey.key_types, 'user_otp_module' => User.otp_modules
  },
  'gems' => %w[activerecord bcrypt bigdecimal rotp actionpack actionview activesupport devise i18n rack-attack turbo-rails rack puma].to_h { |g| [g, Gem.loaded_specs.fetch(g).version.to_s] }
}
# Ruby BigDecimal, as Rails computes with it (rust/src/ruby.rs BigDec). Seeded, so the file is stable.
bd_rng = Random.new(11)
bd_num = lambda do
  digits = Array.new(bd_rng.rand(1..20)) { bd_rng.rand(10).to_s }.join.sub(/\A0+/, '')
  BigDecimal("#{bd_rng.rand < 0.1 ? '-' : ''}0.#{digits.empty? ? '1' : digits}e#{bd_rng.rand(-15..12)}")
end
bigdec = []
[%w[0.4 50000.2], %w[60 50000.2], %w[60 49870.0], %w[1 3], %w[2 3], %w[100 7],
 %w[0.000007999968000127999488002047991808 50000.2], %w[5 0.5], %w[123456789012345678901234567890 7]]
  .each { |a, b| bigdec << ['div', a, b, (BigDecimal(a) / BigDecimal(b)).to_s('F')] }
3000.times { a = bd_num.(); b = bd_num.(); bigdec << ['div', a.to_s('F'), b.to_s('F'), (a / b).to_s('F')] }
500.times { a = bd_num.(); b = bd_num.(); bigdec << ['mul', a.to_s('F'), b.to_s('F'), (a * b).to_s('F')] }
500.times { a = bd_num.(); b = bd_num.(); bigdec << ['add', a.to_s('F'), b.to_s('F'), (a + b).to_s('F')] }
500.times { a = bd_num.(); b = bd_num.(); bigdec << ['sub', a.to_s('F'), b.to_s('F'), (a - b).to_s('F')] }
500.times { a = bd_num.(); bigdec << ['to_f', a.to_s('F'), '', [a.to_f].pack('G').unpack1('H*')] }
500.times { a = bd_num.(); bigdec << ['precision', a.to_s('F'), '', a.precision.to_s] }
[['0.123456789', 8], ['1.99999', 2], ['100', 5], ['0.00049999', 4], ['7.5', 0], ['-0.5', 1], ['49870.0125', 1], ['0.9975', 0]].each do |s, n|
  bigdec << ['floor', s, n.to_s, BigDecimal(s).floor(n).to_d.to_s('F')]
  bigdec << ['ceil', s, n.to_s, BigDecimal(s).ceil(n).to_d.to_s('F')]
end
%w[0.1234567890123456785 0.1234567890123456784 1.0000000000000000005 -0.0000000000000000005].each do |s|
  bigdec << ['round18', s, '', BigDecimal(s).round(18).to_s('F')]
end
vectors['bigdec'] = bigdec
vectors['ruby'] = {
  'iso8601_ms' => [Time.utc(2026, 3, 8, 7, 27, 44, 458_901), Time.utc(2026, 3, 8, 7, 27, 44)].map { |t| [t.iso8601(6), t.as_json] },
  # anchor + offset, then Time#round(6); offsets as IEEE bits so the float is exact on both sides.
  'round6' => [[Time.utc(2026, 9, 1, 10, 0, 0, 123_456), 604_800.0 / 3 * 7],
               [Time.utc(2026, 9, 1, 10, 0, 0), 0.0000005], [Time.utc(2026, 9, 1, 10, 0, 0), 0.0000004999],
               [Time.utc(2026, 9, 1, 10, 0, 0, 1), 86_400.0 / 7]]
                .map { |t, off| [t.iso8601(6), [off].pack('G').unpack1('H*'), (t + off).round(6).iso8601(6)] },
  # Several terms, as Ruby does it: one Time +/- Float per repetition (negative k subtracts).
  'round6_multi' => [
    [Time.utc(2026, 9, 1, 10, 0, 0, 123_456), [[604_800.0 / 3 * 7, 3]]],
    [Time.utc(2026, 9, 1, 10, 0, 0, 999_999), [[86_400.0 / 7, -2]]],
    [Time.utc(2026, 9, 1, 10, 0, 0), [[0.1, 1], [86_400.0 / 7, 2]]],
    [Time.utc(2026, 9, 1, 10, 0, 0, 5), [[0.0000005, 1], [-0.0000004999, 1], [3600.0 / 11, -3]]],
    [Time.utc(2026, 9, 1, 10, 0, 0), [[1.0 / 3, 3], [0.2, 5]]]
  ].map do |t, terms|
    r = terms.reduce(t) { |acc, (f, k)| k.abs.times.reduce(acc) { |a, _| k.positive? ? a + f : a - f } }
    [t.iso8601(6), terms.map { |f, k| [[f].pack('G').unpack1('H*'), k] }, r.round(6).iso8601(6)]
  end,
  # exceeds: anchor + k*f > now, on the exact boundary and one microsecond either side.
  'exceeds' => [[Time.utc(2026, 9, 1, 10, 0, 0), 90.5, 3], [Time.utc(2026, 9, 1, 10, 0, 0, 250_000), 0.25, 4],
                [Time.utc(2026, 9, 1, 10, 0, 0, 123_456), 86_400.0 / 7, 1], [Time.utc(2026, 9, 1, 10, 0, 0), 0.1, 3]].flat_map do |t, f, k|
    due = k.times.reduce(t) { |a, _| a + f }
    [-1, 0, 1].map do |d|
      now = due.round(6) + Rational(d, 1_000_000)
      [t.iso8601(6), [f].pack('G').unpack1('H*'), k, now.iso8601(6), due > now]
    end
  end,
  'to_sentence' => [%w[a], %w[a b], %w[a b c]].map { |a| [a, a.to_sentence] },
  'inspect' => [['EGeneral:Internal error'], ['EOrder:Insufficient funds', 'a "quoted" one']].map { |a| [a, a.inspect] }
}
require 'active_support/testing/time_helpers'
include ActiveSupport::Testing::TimeHelpers
# Unsaved bots: the schedule only reads started_at and settings. Every case is recorded through the real
# model methods (Automation::Schedulable, Bot::SmartIntervalable, Bot::Accountable's interval count).
schedule_cases = []
anchors = [Time.utc(2026, 1, 31, 10, 0, 0, 123_456), Time.utc(2026, 9, 1, 22, 1, 3), Time.utc(2026, 2, 28, 23, 59, 59, 999_999)]
settings = [
  { 'interval' => 'hour', 'quote_amount' => 10.0 }, { 'interval' => 'day', 'quote_amount' => 10.0 },
  { 'interval' => 'week', 'quote_amount' => 60.0 }, { 'interval' => 'month', 'quote_amount' => 100.0 },
  { 'interval' => 'week', 'quote_amount' => 60.0, 'smart_intervaled' => true, 'smart_interval_quote_amount' => 30.0 },
  { 'interval' => 'week', 'quote_amount' => 100.0, 'smart_intervaled' => true, 'smart_interval_quote_amount' => 7.0 },
  { 'interval' => 'month', 'quote_amount' => 100.0, 'smart_intervaled' => true, 'smart_interval_quote_amount' => 100.0 },
  { 'interval' => 'month', 'quote_amount' => 100.0, 'smart_intervaled' => true, 'smart_interval_quote_amount' => 33.0 },
  { 'interval' => 'day', 'quote_amount' => 10.0, 'smart_intervaled' => false, 'smart_interval_quote_amount' => 3.0 }
]
offsets = [-3600, 0, 1, 3599.9999995, 86_400 * 3 + 7, 86_400 * 45, 86_400 * 400 + 0.5]
record = lambda do |anchor, s, now, bot|
  travel_to(now, with_usec: true) do
    nxt = bot.next_interval_checkpoint_at
    last = bot.last_interval_checkpoint_at
    count = ((last.round(6) - anchor.round(6)) / bot.effective_interval_duration).floor + 1
    schedule_cases << { 'anchor' => anchor.iso8601(6), 'now' => now.iso8601(6), 'settings' => s,
                        'next' => nxt.round(6).iso8601(6), 'last' => last.round(6).iso8601(6), 'count' => count }
    nxt
  end
end
anchors.each do |anchor|
  settings.each do |s|
    bot = Bots::DcaMultiAsset.new(started_at: anchor, settings: s)
    offsets.each { |off| record.call(anchor, s, anchor + off, bot) }
    # On the grid: the bot's own first two checkpoints after the anchor, each exactly on, and one microsecond either side.
    grid = []
    point = anchor
    2.times { point = travel_to(point + Rational(1, 1_000_000), with_usec: true) { bot.next_interval_checkpoint_at.round(6) }; grid << point }
    grid.each { |g| [-1, 0, 1].each { |d| record.call(anchor, s, g + Rational(d, 1_000_000), bot) } }
  end
end
vectors['schedule'] = schedule_cases
kraken = Exchanges::Kraken.new
sizing = []
sizing_tickers = [
  { minimum_base_size: '0.00005', minimum_quote_size: '0.5', base_decimals: 8, quote_decimals: 5, price_decimals: 1 },
  { minimum_base_size: '0.0001', minimum_quote_size: '5', base_decimals: 8, quote_decimals: 2, price_decimals: 2 },
  { minimum_base_size: '1', minimum_quote_size: '500', base_decimals: 0, quote_decimals: 0, price_decimals: 0 }
]
%w[50000.2 49995.37 0.00123456 1234567.8].each do |price_s|
  %w[60 0.4 5.00001 0.0001 123.456789].each do |x_s|
    sizing_tickers.each do |t|
      %i[market_order limit_order].each do |order_type|
        ticker = Ticker.new(exchange: kraken, **t.transform_values { |v| v.is_a?(String) ? BigDecimal(v) : v })
        bot = Bots::DcaMultiAsset.new(exchange: kraken)
        price = order_type == :limit_order ? ticker.adjusted_price(price: BigDecimal(price_s) * (1.to_d - 0.0025.to_d)) : BigDecimal(price_s)
        next if price.zero? # both refuse before sizing ("limit price rounds to zero..."), covered by unit tests on each side
        x = BigDecimal(x_s)
        info = bot.send(:calculate_best_amount_info, { ticker:, price:, amount: x / price, quote_amount: x, side: :buy, order_type: })
        volume = ticker.adjusted_amount(amount: info[:amount], amount_type: info[:amount_type])
        sizing << { 'ticker' => t.transform_values(&:to_s), 'last_or_ask' => price_s, 'x' => x_s, 'order_type' => order_type.to_s,
                    'price' => price.to_s('F'), 'amount' => (x / price).to_s('F'), 'amount_type' => info[:amount_type].to_s,
                    'below_minimum' => info[:below_minimum_amount], 'volume' => volume.to_d.to_s('F') }
      end
    end
  end
end
vectors['sizing'] = sizing
# Exchange#failure_kind / #transient_error? / #throttled_error? per venue (rust/src/engine/venue_rules.rs).
failure_messages = ['insufficient buying power', 'unauthorized.', 'HTTP 401', 'HTTP 403', 'forbidden.', 'rate limit exceeded',
                    'Faraday::ConnectionFailed: Connection refused - connect(2) for "paper-api.alpaca.markets" port 443',
                    'Faraday::TimeoutError: Net::ReadTimeout', 'internal server error', 'Connection reset by peer',
                    'EOrder:Insufficient funds', 'EAPI:Invalid key', 'EAPI:Rate limit exceeded', 'EService:Unavailable',
                    'EAPI:Invalid nonce', 'qty must be >= 0.000027', 'EAPI:Invalid signature', 'EGeneral:Permission denied',
                    'EAccount:Invalid permissions:USDT trading restricted for AT.', 'EGeneral:Internal error', 'EService:Busy',
                    'EService:Deadline elapsed']
vectors['failure_kinds'] = { 'Exchanges::Alpaca' => Exchanges::Alpaca.new, 'Exchanges::Kraken' => Exchanges::Kraken.new }.flat_map do |type, ex|
  failure_messages.map { |m| [type, m, ex.failure_kind([m])&.to_s, ex.transient_error?([m]), ex.throttled_error?([m])] }
end
# The Ruby the engine ports by hand (the Alpaca venue, the carry, and the two poll jobs that used to draw it down).
# rust/tests/venue_rules.rs fails when any of it changes, until re-recorded and re-checked: a later change to Rails' carry trips it.
vectors['ported_sources'] = %w[app/models/clients/alpaca.rb app/models/exchanges/alpaca.rb app/models/client.rb app/models/exchange.rb
                                 app/models/exchanges/kraken.rb app/models/bot/accountable.rb
                                 app/jobs/bot/fetch_and_update_open_orders_job.rb app/jobs/bot/fetch_and_update_order_job.rb]
                              .to_h { |f| [f, Digest::SHA256.file(Rails.root.join(f)).hexdigest] }
# Exchanges::Alpaca sizing and the exact strings #set_market_order / #set_limit_order send (rust/src/engine/amount.rs).
alpaca = Exchanges::Alpaca.new
captured = nil
alpaca.instance_variable_set(:@client, Object.new.tap do |c|
  c.define_singleton_method(:create_order) { |**kw| captured = kw; Result::Success.new('id' => 'X') }
end)
crypto = Asset.new(symbol: 'BTC', category: 'Cryptocurrency') # crypto_ticker? reads the base asset's category
alpaca_sizing = []
alpaca_tickers = [
  { minimum_base_size: '0.000027', minimum_quote_size: '1', base_decimals: 9, quote_decimals: 2, price_decimals: 0 },
  { minimum_base_size: '0.0001', minimum_quote_size: '1', base_decimals: 4, quote_decimals: 2, price_decimals: 2 },
  { minimum_base_size: '1', minimum_quote_size: '10', base_decimals: 0, quote_decimals: 2, price_decimals: 5 },
  # >17 significant digits after flooring (qty ~8.1e8 at 9 decimals): the only ticker where Float formatting changes a string
  { minimum_base_size: '0.000000001', minimum_quote_size: '1', base_decimals: 9, quote_decimals: 2, price_decimals: 8 }
]
%w[64321.5 0.00123456 1.5 123456789012.12345].each do |price_s|
  %w[60 0.99 1 5.005 123.456789 1000000].each do |x_s|
    alpaca_tickers.each do |t|
      %i[market_order limit_order].each do |order_type|
        ticker = Ticker.new(exchange: alpaca, ticker: 'BTC/USD', base: 'BTC', quote: 'USD', base_asset: crypto,
                            **t.transform_values { |v| v.is_a?(String) ? BigDecimal(v) : v })
        bot = Bots::DcaMultiAsset.new(exchange: alpaca)
        price = order_type == :limit_order ? ticker.adjusted_price(price: BigDecimal(price_s) * (1.to_d - 0.0025.to_d)) : BigDecimal(price_s)
        next if price.zero? # both refuse before sizing ("limit price rounds to zero")
        x = BigDecimal(x_s)
        info = bot.send(:calculate_best_amount_info, { ticker:, price:, amount: x / price, quote_amount: x, side: :buy, order_type: })
        captured = nil
        if order_type == :limit_order
          alpaca.limit_buy(ticker:, amount: info[:amount], amount_type: info[:amount_type], price:)
        else
          alpaca.market_buy(ticker:, amount: info[:amount], amount_type: info[:amount_type])
        end
        alpaca_sizing << { 'ticker' => t.transform_values(&:to_s), 'last_or_ask' => price_s, 'x' => x_s, 'order_type' => order_type.to_s,
                           'price' => price.to_s('F'), 'amount' => (x / price).to_s('F'), 'amount_type' => info[:amount_type].to_s,
                           'below_minimum' => info[:below_minimum_amount], 'wire' => captured.transform_keys(&:to_s).transform_values(&:to_s) }
      end
    end
  end
end
vectors['alpaca_sizing'] = alpaca_sizing
# Exchanges::Alpaca#parse_order_data over documented order shapes and every status it maps (rust/src/venue/alpaca.rs).
alpaca_parser = Exchanges::Alpaca.new
order_shapes = [
  { 'type' => 'market', 'side' => 'buy', 'notional' => '60', 'qty' => nil, 'filled_qty' => '0', 'filled_avg_price' => nil, 'limit_price' => nil },
  { 'type' => 'market', 'side' => 'buy', 'notional' => '60', 'qty' => nil, 'filled_qty' => '0.000932719', 'filled_avg_price' => '64328.1', 'limit_price' => nil },
  { 'type' => 'limit', 'side' => 'buy', 'notional' => nil, 'qty' => '0.00093525', 'filled_qty' => '0.0004', 'filled_avg_price' => '64149.97', 'limit_price' => '64149.97' }
]
order_statuses = %w[new accepted pending_new filled canceled expired replaced rejected partially_filled done_for_day pending_cancel held mystery]
dec_s = ->(d) { d.nil? ? nil : d.to_d.to_s('F') }
vectors['alpaca_orders'] = order_statuses.product(order_shapes).map do |status, shape|
  body = shape.merge('id' => 'O1', 'symbol' => 'BTC/USD', 'status' => status)
  parsed = alpaca_parser.send(:parse_order_data, body)
  [body, { 'status' => parsed[:status].to_s, 'price' => dec_s.(parsed[:price]), 'amount' => dec_s.(parsed[:amount]),
           'quote_amount' => dec_s.(parsed[:quote_amount]), 'amount_exec' => dec_s.(parsed[:amount_exec]),
           'quote_amount_exec' => dec_s.(parsed[:quote_amount_exec]), 'order_type' => parsed[:order_type].to_s, 'side' => parsed[:side].to_s }]
end
# Bot::Accountable#pending_quote_amount with Rails' fill-credit fix: a cancelled or abandoned REGULAR
# buy counts what it filled, nothing in polling moves the carry, and every counted row comes from one read. Generated rows on a
# bot inserted directly, inside a transaction that is rolled back, so the development database keeps nothing.
CARRY_KINDS = %w[closed open_limit unknown_market cancelled_partial cancelled_unfilled abandoned_nil abandoned_partial
                 failed skipped sell rebalance before_window].freeze
carry_row = lambda do |kind, n|
  row = { 'status' => 0, 'side' => 0, 'transaction_type' => 'REGULAR', 'external_id' => "rust-carry-#{n}", 'order_type' => 0,
          'price' => '64150', 'created_at' => "2026-09-0#{2 + (n % 5)} 10:00:00" }
  case kind
  when 'closed' then row.merge('external_status' => 2, 'quote_amount' => '60', 'quote_amount_exec' => '59.97', 'amount_exec' => '0.000935')
  when 'open_limit' then row.merge('external_status' => 1, 'order_type' => 1, 'amount' => '0.000935', 'amount_exec' => '0.0003',
                                   'quote_amount_exec' => '19.245')
  when 'unknown_market' then row.merge('external_status' => 0, 'quote_amount' => '60')
  when 'cancelled_partial' then row.merge('external_status' => 3, 'quote_amount' => '60', 'quote_amount_exec' => '39.96', 'amount_exec' => '0.000623')
  when 'cancelled_unfilled' then row.merge('external_status' => 3, 'quote_amount' => '60', 'quote_amount_exec' => '0', 'amount_exec' => '0')
  when 'abandoned_nil' then row.merge('external_status' => 4, 'quote_amount' => '60')
  when 'abandoned_partial' then row.merge('external_status' => 4, 'quote_amount' => '60', 'quote_amount_exec' => '12.5', 'amount_exec' => '0.000195')
  when 'failed' then row.merge('status' => 1, 'external_id' => nil, 'quote_amount' => '60', 'quote_amount_exec' => '0', 'amount_exec' => '0')
  when 'skipped' then row.merge('status' => 2, 'external_id' => nil, 'quote_amount' => '0.4', 'quote_amount_exec' => '0', 'amount_exec' => '0')
  when 'sell' then row.merge('side' => 1, 'external_status' => 2, 'quote_amount' => '60', 'quote_amount_exec' => '60', 'amount_exec' => '0.000935')
  when 'rebalance' then row.merge('transaction_type' => 'REBALANCE', 'external_status' => 3, 'quote_amount' => '60', 'quote_amount_exec' => '30',
                                  'amount_exec' => '0.00047')
  when 'before_window' then row.merge('external_status' => 3, 'quote_amount' => '60', 'quote_amount_exec' => '40', 'amount_exec' => '0.000623',
                                      'created_at' => '2026-08-31 10:00:00')
  end
end
carry_rng = Random.new(2_202_615)
vectors['carry'] = Array.new(36) do |i|
  settings = { 'interval' => %w[day week].fetch(i % 2), 'quote_amount' => 60.0 }
  settings.merge!('smart_intervaled' => true, 'smart_interval_quote_amount' => 20.0) if i % 6 == 5
  kinds = Array.new(carry_rng.rand(0..5)) { CARRY_KINDS.sample(random: carry_rng) }
  kinds |= ['cancelled_partial'] if i.even? # half the cases exercise the cancelled fill's credit
  c = { 'settings' => settings, 'started_at' => '2026-09-01 10:00:00', 'settings_changed_at' => (i % 4 == 3 ? '2026-09-03 12:00:00' : nil),
        'carry' => %w[0 12.5 100.0].fetch(i % 3), 'rows' => kinds.each_with_index.map { |k, n| carry_row.(k, (i * 10) + n) },
        'now' => (Time.utc(2026, 9, 1, 10) + ((i % 9) + 1).days + 1).iso8601(6) }
  ActiveRecord::Base.transaction do
    alpaca = Exchanges::Alpaca.first || Exchanges::Alpaca.create!(name: 'Alpaca', maker_fee: '0.15', taker_fee: '0.25')
    user = User.new(name: 'Carry', email: 'rust-carry@example.com', password: 'correct horse battery staple', confirmed_at: Time.current)
    user.save!(validate: false)
    stamp = Time.current
    id = Bot.insert!({ 'type' => 'Bots::DcaMultiAsset', 'label' => 'Carry', 'user_id' => user.id, 'exchange_id' => alpaca.id, 'status' => Bot.statuses[:scheduled],
                       'settings' => settings, 'transient_data' => { 'missed_quote_amount' => c['carry'] }, 'started_at' => c['started_at'],
                       'settings_changed_at' => c['settings_changed_at'], 'created_at' => stamp, 'updated_at' => stamp }).rows.first.first
    c['rows'].each do |r|
      Transaction.insert!(r.merge('bot_id' => id, 'exchange_id' => alpaca.id, 'bot_interval' => settings['interval'], 'bot_quote_amount' => 60,
                                  'error_messages' => [], 'updated_at' => r['created_at']))
    end
    c['pending'] = travel_to(Time.iso8601(c['now']), with_usec: true) { Bot.find(id).pending_quote_amount.to_d.to_s('F') }
    raise ActiveRecord::Rollback
  end
  c
end
# Alpaca crypto baskets (rust/src/engine/basket.rs), recorded on real rows inside a transaction that is rolled back,
# so the development database keeps nothing. Members are V-prefixed so no real asset, ticker or order id is touched.
Rails.cache = ActiveSupport::Cache::NullStore.new # metrics(force: true) recomputes; nothing is written to a cache store
BASKET_PAIRS = {
  'VBTC' => { 'base_decimals' => 9, 'quote_decimals' => 2, 'price_decimals' => 2, 'minimum_base_size' => '0.000027', 'minimum_quote_size' => '1' },
  'VETH' => { 'base_decimals' => 9, 'quote_decimals' => 2, 'price_decimals' => 2, 'minimum_base_size' => '0.0005', 'minimum_quote_size' => '1' },
  'VSOL' => { 'base_decimals' => 9, 'quote_decimals' => 2, 'price_decimals' => 3, 'minimum_base_size' => '0.01', 'minimum_quote_size' => '1' },
  'VADA' => { 'base_decimals' => 9, 'quote_decimals' => 2, 'price_decimals' => 4, 'minimum_base_size' => '1', 'minimum_quote_size' => '1' }
}.freeze
# A basket over `weights` on the development database's Alpaca exchange, saved as BotApi::Bots::Create saves one (its
# after_save refresh_composition writes bot_index_assets), with `rows` inserted as REGULAR buys. Yields it and its assets
# by symbol, returns the block's value, and rolls everything back.
def with_basket(weights, settings: {}, rows: [])
  out = nil
  ActiveRecord::Base.transaction do
    alpaca = Exchanges::Alpaca.first || Exchanges::Alpaca.create!(name: 'Alpaca', maker_fee: '0.15', taker_fee: '0.25')
    usd = Asset.create!(external_id: 'rust-vector-usd', symbol: 'USD', name: 'US Dollar', category: 'Currency')
    ExchangeAsset.create!(exchange: alpaca, asset: usd, available: true) # Ticker#exchange_matches_assets
    assets = weights.keys.to_h do |sym|
      asset = Asset.create!(external_id: "rust-vector-#{sym.downcase}", symbol: sym, name: sym, category: 'Cryptocurrency')
      ExchangeAsset.create!(exchange: alpaca, asset:, available: true)
      Ticker.create!(exchange: alpaca, ticker: "#{sym}/USD", base: sym, quote: 'USD', base_asset: asset, quote_asset: usd,
                     **BASKET_PAIRS.fetch(sym).to_h { |k, v| [k.to_sym, v.is_a?(String) ? BigDecimal(v) : v] })
      [sym, asset]
    end
    user = User.new(name: 'Vectors', email: 'rust-vectors@example.com', password: 'correct horse battery staple', confirmed_at: Time.current)
    user.save!(validate: false)
    bot = user.bots.new(type: 'Bots::DcaMultiAsset', exchange: alpaca, settings: {
      'quote_asset_id' => usd.id, 'quote_amount' => 60.0, 'interval' => 'day', 'weighting' => 'manual',
      'allocations' => weights.to_h { |sym, w| [assets.fetch(sym).id.to_s, w] }
    }.merge(settings))
    bot.set_missed_quote_amount
    bot.save!
    rows.each do |r|
      Transaction.insert!(r.except('asset').merge('bot_id' => bot.id, 'exchange_id' => alpaca.id, 'base_asset_id' => assets.fetch(r['asset']).id,
                                                  'quote_asset_id' => usd.id, 'base' => r['asset'], 'quote' => 'USD', 'side' => 0,
                                                  'transaction_type' => 'REGULAR', 'bot_interval' => 'day', 'bot_quote_amount' => 60,
                                                  'error_messages' => [], 'updated_at' => r['created_at']))
    end
    out = yield bot.reload, assets, alpaca
    raise ActiveRecord::Rollback
  end
  out
end
vector_row_id = 0
LEDGER_KINDS = %w[closed closed_nil_exec closed_zero_quote_exec open_partial open_unfilled unknown_market cancelled_partial abandoned failed skipped].freeze
# One REGULAR buy of `sym` in the state `kind`, priced near the member's usual price. Every decimal is a string, as the
# Rust test inserts it; Rails casts it to BigDecimal and binds it as a Float, as for any row.
ledger_row = lambda do |rng, sym, kind|
  vector_row_id += 1
  p = ({ 'VBTC' => 64_000, 'VETH' => 2500, 'VSOL' => 150 }.fetch(sym) * (0.9 + (rng.rand(20) / 100.0))).round(2).to_d
  q = (10 + rng.rand(200)).to_d
  a = (q / p).round(9)
  row = { 'asset' => sym, 'status' => 0, 'external_id' => "rust-vector-#{vector_row_id}", 'order_type' => 0, 'price' => p.to_s('F'),
          'created_at' => "2026-08-#{10 + (vector_row_id % 18)} 10:00:00" }
  case kind
  when 'closed' then row.merge('external_status' => 2, 'quote_amount' => q.to_s('F'), 'amount_exec' => a.to_s('F'), 'quote_amount_exec' => (a * p).to_s('F'))
  when 'closed_nil_exec' then row.merge('external_status' => 2, 'amount' => a.to_s('F'))
  when 'closed_zero_quote_exec' then row.merge('external_status' => 2, 'quote_amount' => q.to_s('F'), 'amount_exec' => a.to_s('F'), 'quote_amount_exec' => '0')
  when 'open_partial' then row.merge('external_status' => 1, 'order_type' => 1, 'amount' => a.to_s('F'), 'amount_exec' => (a / 3).round(9).to_s('F'),
                                     'quote_amount_exec' => ((a / 3).round(9) * p).to_s('F'))
  when 'open_unfilled' then row.merge('external_status' => 0, 'order_type' => 1, 'amount' => a.to_s('F'), 'amount_exec' => '0', 'quote_amount_exec' => '0')
  when 'unknown_market' then row.merge('external_status' => 0, 'quote_amount' => q.to_s('F'))
  when 'cancelled_partial' then row.merge('external_status' => 3, 'quote_amount' => q.to_s('F'), 'amount_exec' => (a / 2).round(9).to_s('F'),
                                          'quote_amount_exec' => ((a / 2).round(9) * p).to_s('F'))
  when 'abandoned' then row.merge('external_status' => 4, 'quote_amount' => q.to_s('F'))
  when 'failed' then row.merge('status' => 1, 'external_id' => nil, 'quote_amount' => q.to_s('F'), 'amount_exec' => '0', 'quote_amount_exec' => '0')
  when 'skipped' then row.merge('status' => 2, 'external_id' => nil, 'quote_amount' => q.to_s('F'), 'amount_exec' => '0', 'quote_amount_exec' => '0')
  when 'cancelled_limit_partial' then row.merge('external_status' => 3, 'order_type' => 1, 'amount' => a.to_s('F'), 'amount_exec' => (a / 2).round(9).to_s('F'),
                                                'quote_amount_exec' => ((a / 2).round(9) * p).to_s('F'))
  when 'abandoned_limit' then row.merge('external_status' => 4, 'order_type' => 1, 'amount' => a.to_s('F'))
  when 'failed_resting' then row.merge('status' => 1, 'external_status' => 1, 'order_type' => 1, 'amount' => a.to_s('F'), 'amount_exec' => (a / 3).round(9).to_s('F'),
                                       'quote_amount_exec' => ((a / 3).round(9) * p).to_s('F'))
  when 'skipped_closed' then row.merge('status' => 2, 'external_status' => 2, 'quote_amount' => q.to_s('F'), 'amount' => a.to_s('F'), 'amount_exec' => a.to_s('F'),
                                       'quote_amount_exec' => (a * p).to_s('F'))
  end
end
pairs_of = ->(weights) { BASKET_PAIRS.slice(*weights.keys) }
# Bot::Composition::Measurable#metrics' asset_breakdown amounts and #reserved_waiting_amounts(:buy), by member symbol.
ledger_rng = Random.new(2_202_610)
ledger_cases = Array.new(40) do
  members = %w[VBTC VETH VSOL].first(ledger_rng.rand(1..3))
  weights = members.size == 1 ? { members[0] => 1.0 } : Bots::DcaMultiAsset.new.send(:normalize_allocations, members.to_h { |m| [m, 1.0] })
  rows = members.flat_map { |sym| Array.new(ledger_rng.rand(0..4)) { ledger_row.(ledger_rng, sym, LEDGER_KINDS.sample(random: ledger_rng)) } }
  { weights:, rows: }
end
# Rows the random cases never hold, each one decisive for a filter: a cancelled or abandoned limit buy whose amount is
# set (a resting remainder only the external-status filter keeps out), and a failed or skipped row carrying amounts (kept
# out only by the submitted filter). Drawn after the random cases so those stay as they were.
filter_rng = Random.new(2_202_611)
FILTER_KINDS = %w[cancelled_limit_partial abandoned_limit failed_resting skipped_closed].freeze
ledger_cases += FILTER_KINDS.map { |kind| { weights: { 'VBTC' => 1.0 }, rows: [ledger_row.(filter_rng, 'VBTC', 'open_partial'), ledger_row.(filter_rng, 'VBTC', kind)] } }
ledger_cases << { weights: Bots::DcaMultiAsset.new.send(:normalize_allocations, { 'VBTC' => 1.0, 'VETH' => 1.0 }),
                  rows: FILTER_KINDS.flat_map { |kind| %w[VBTC VETH].map { |sym| ledger_row.(filter_rng, sym, kind) } } }
vectors['basket_ledgers'] = ledger_cases.map do |c|
  with_basket(c[:weights], rows: c[:rows]) do |bot, assets, _alpaca|
    m = bot.metrics(force: true)
    breakdown = m[:asset_breakdown] || {}
    symbol_of = ->(id) { assets.find { |_, a| a.id == id }&.first }
    { 'weights' => c[:weights], 'pairs' => pairs_of.(c[:weights]), 'rows' => c[:rows],
      'holdings' => assets.filter_map { |sym, a| (h = breakdown.dig(bot.key_for(a.id, m), :amount)) && [sym, h.to_d.to_s('F')] }.to_h,
      'reserved' => bot.send(:reserved_waiting_amounts, :buy).to_h { |id, amount| [symbol_of.(id), amount.to_d.to_s('F')] } }
  end
end

# The weights a basket is derived with: settings sliders (Floats), renormalised in Float with Ruby's compensated Array#sum,
# written into bot_index_assets.target_allocation decimal(10,6), and read back as BigDecimal, then #to_f.
bits = ->(f) { [f].pack('G').unpack1('H*') }
composition_rng = Random.new(2_202_640)
weight_sets = [{ 'VBTC' => 1.0 }, { 'VBTC' => 0.5, 'VETH' => 0.5 }, { 'VBTC' => 0.7, 'VETH' => 0.3 },
               { 'VBTC' => 0.334, 'VETH' => 0.333, 'VSOL' => 0.333 }, { 'VBTC' => 0.5, 'VETH' => 0.3, 'VSOL' => 0.2 },
               { 'VBTC' => 0.1, 'VETH' => 0.2, 'VSOL' => 0.3 }, { 'VBTC' => 0.6, 'VETH' => 0.4, 'VSOL' => 0.0 }] +
              Array.new(12) { Bots::DcaMultiAsset.new.send(:normalize_allocations, %w[VBTC VETH VSOL].to_h { |m| [m, composition_rng.rand(1..97).to_f] }) }
vectors['float_sum'] = (weight_sets.map(&:values) + [[0.1, 0.2, 0.3], [1.0e16, 1.0, -1.0e16], [0.333, 0.333, 0.334]])
                       .map { |values| [values.map(&bits), bits.(values.sum)] }
target_type = BotIndexAsset.type_for_attribute(:target_allocation)
derived = weight_sets.flat_map { |w| (t = w.values.sum).positive? ? w.values.map { |v| v / t } : [] }
vectors['decimal_10_6'] = (derived + [1.0 / 3, 2.0 / 3, 0.3333335, 0.1234565, 0.0000005, 0.9999995, 1.0e-7, 0.12345649999999999])
                          .map { |f| [bits.(f), target_type.cast(f).to_s('F')] }
composition_cases = [[weight_sets[0], []], [weight_sets[1], []], [weight_sets[1], %w[VETH]], [weight_sets[1], %w[VBTC VETH]],
                     [weight_sets[2], []], [weight_sets[2], %w[VBTC]], [weight_sets[3], []], [weight_sets[3], %w[VSOL]],
                     [weight_sets[4], []], [weight_sets[4], %w[VETH]], [weight_sets[5], []], [weight_sets[6], []]] +
                    weight_sets.drop(7).flat_map { |w| [[w, []], [w, [w.keys.sample(random: composition_rng)]]] }
# Members left whose stored decimal(10,6) targets do not sum to 1, so buyable_allocations re-weights them in BigDecimal: a
# four-asset basket with one member out (0.444444 + 0.333333 + 0.222222), and true thirds (0.333333 × 3). Then members that
# exited and trade again: the third refresh re-adds them (`readd`).
four = { 'VBTC' => 0.4, 'VETH' => 0.3, 'VSOL' => 0.2, 'VADA' => 0.1 }
thirds = { 'VBTC' => 1.0 / 3, 'VETH' => 1.0 / 3, 'VSOL' => 1.0 / 3 }
composition_cases = composition_cases.map { |w, u| [w, u, []] } +
                    [[four, %w[VADA], []], [four, %w[VETH], []], [thirds, [], []], [thirds, %w[VSOL], []],
                     [weight_sets[4], %w[VETH], %w[VETH]], [four, %w[VADA VSOL], %w[VSOL]], [thirds, %w[VBTC], %w[VBTC]]]
vectors['basket_compositions'] = composition_cases.map do |weights, untradable, readd|
  with_basket(weights) do |bot, assets, alpaca|
    failure = nil
    ticker_of = ->(sym) { Ticker.find_by!(exchange: alpaca, base_asset: assets.fetch(sym)) }
    entered = bot.bot_index_assets.to_h { |b| [b.asset_id, b.entered_at] }
    if untradable.any?
      untradable.each { |sym| ticker_of.(sym).update_columns(trading_enabled: false) }
      result = bot.refresh_composition
      failure = result.errors.to_sentence if result.failure?
    end
    if readd.any?
      readd.each { |sym| ticker_of.(sym).update_columns(trading_enabled: true) }
      bot.refresh_composition.then { |r| raise r.errors.to_sentence if r.failure? }
    end
    # Per row: entered_at as the first save wrote it, and exited_at blank.
    stamps = bot.bot_index_assets.reload.order(:id).map { |b| [assets.key(b.asset), b.entered_at == entered[b.asset_id], b.exited_at.nil?] }
    { 'weights' => weights, 'pairs' => pairs_of.(weights), 'exchange' => alpaca.name, 'untradable' => untradable, 'readd' => readd,
      'failure' => failure, 'stamps' => stamps,
      'index_rows' => bot.bot_index_assets.order(:id).map { |b| [assets.key(b.asset), b.target_allocation&.to_s('F'), b.in_index] },
      'members' => bot.send(:buyable_allocations).map { |a| [assets.key(a[:asset]), bits.(a[:target_allocation].to_f)] } }
  end
end

# Bot::Composition::OrderSetter#get_orders_data over recorded holdings and prices: every order it returns (base, price,
# base amount, quote amount) or its failure. Prices are read from the book below, not from the venue.
module VectorPrices
  mattr_accessor :book
  def get_ask_price(force: false) = Result::Success.new(VectorPrices.book.fetch([id, :ask]))
  def get_last_price(force: false) = Result::Success.new(VectorPrices.book.fetch([id, :last]))
end
Ticker.prepend(VectorPrices)
closed_row = lambda do |sym, value, price|
  vector_row_id += 1
  qty = (value.to_d / price.to_d).round(9)
  { 'asset' => sym, 'status' => 0, 'external_status' => 2, 'external_id' => "rust-vector-#{vector_row_id}", 'order_type' => 0,
    'price' => price.to_s, 'quote_amount' => value.to_s, 'amount_exec' => qty.to_s('F'), 'quote_amount_exec' => (qty * price.to_d).to_s('F'),
    'created_at' => '2026-08-20 10:00:00' }
end
split_rng = Random.new(2_202_650)
base_prices = { 'VBTC' => 64_000.0, 'VETH' => 2500.0, 'VSOL' => 150.0 }
usual = { 'VBTC' => { 'ask' => '64000', 'last' => '63990' }, 'VETH' => { 'ask' => '2500', 'last' => '2499.5' }, 'VSOL' => { 'ask' => '150', 'last' => '149.9' } }
split_cases = Array.new(48) do
  members = %w[VBTC VETH VSOL].first(split_rng.rand(1..3))
  weights = members.size == 1 ? { members[0] => 1.0 } : Bots::DcaMultiAsset.new.send(:normalize_allocations, members.to_h { |m| [m, split_rng.rand(1..9).to_f] })
  rows = members.flat_map { |sym| Array.new(split_rng.rand(0..3)) { ledger_row.(split_rng, sym, LEDGER_KINDS.sample(random: split_rng)) } }
  prices = members.to_h { |m| p = base_prices[m] * (0.8 + (split_rng.rand(40) / 100.0)); [m, { 'ask' => format('%.3f', p), 'last' => format('%.3f', p * 0.999) }] }
  { weights:, rows:, prices:, limit: split_rng.rand < 0.5, x: %w[0.5 3 60 120 123.45 1000].sample(random: split_rng) }
end
split_cases += [
  # A limit price under the pair's precision: Rails fails before any order ("limit price rounds to zero at 3 decimals").
  { weights: { 'VBTC' => 0.5, 'VSOL' => 0.5 }, rows: [], prices: { 'VBTC' => usual['VBTC'], 'VSOL' => { 'ask' => '0.0004', 'last' => '0.0004' } }, limit: true, x: '60' },
  # At balance: each member holds its weight of 600 at the ask, so the contribution is spent by weight.
  { weights: { 'VBTC' => 0.334, 'VETH' => 0.333, 'VSOL' => 0.333 },
    rows: [closed_row.('VBTC', 200.4, 64_000), closed_row.('VETH', 199.8, 2500), closed_row.('VSOL', 199.8, 150)], prices: usual, limit: false, x: '60' },
  # Drifted past its share even after the contribution: that member's offset is zero, the other takes everything.
  { weights: { 'VBTC' => 0.5, 'VETH' => 0.5 }, rows: [closed_row.('VBTC', 1000, 64_000)], prices: usual.slice('VBTC', 'VETH'), limit: false, x: '60' },
  # One member: the offsets reduce to the contribution.
  { weights: { 'VBTC' => 1.0 }, rows: [closed_row.('VBTC', 300, 64_000)], prices: usual.slice('VBTC'), limit: true, x: '123.45' },
  # True thirds, stored as 0.333333 each and re-weighted in BigDecimal, valued at limit prices.
  { weights: { 'VBTC' => 1.0 / 3, 'VETH' => 1.0 / 3, 'VSOL' => 1.0 / 3 }, rows: [closed_row.('VBTC', 100, 64_000)], prices: usual, limit: true, x: '60' },
  # A resting limit buy counts as held: VETH's unfilled 0.08 at 2500 balances VBTC's 200, so the contribution splits by
  # weight instead of all going to VETH.
  { weights: { 'VBTC' => 0.5, 'VETH' => 0.5 },
    rows: [closed_row.('VBTC', 200, 64_000),
           { 'asset' => 'VETH', 'status' => 0, 'external_status' => 0, 'external_id' => "rust-vector-#{vector_row_id += 1}", 'order_type' => 1,
             'price' => '2500.0', 'amount' => '0.08', 'amount_exec' => '0', 'quote_amount_exec' => '0', 'created_at' => '2026-08-20 10:00:00' }],
    prices: usual.slice('VBTC', 'VETH'), limit: false, x: '60' },
  # 7:7:1 from nothing, stored as 0.466667, 0.466667 and 0.066667 and re-weighted: the offsets sum past the contribution,
  # and the last leg is capped by what is left rather than by its own share.
  { weights: { 'VBTC' => 7.0 / 15, 'VETH' => 7.0 / 15, 'VSOL' => 1.0 / 15 }, rows: [], prices: usual, limit: false, x: '60' }
]
vectors['basket_splits'] = split_cases.map do |c|
  settings = c[:limit] ? { 'limit_ordered' => true, 'limit_order_pcnt_distance' => 0.0025 } : {}
  with_basket(c[:weights], settings:, rows: c[:rows]) do |bot, assets, alpaca|
    VectorPrices.book = c[:prices].each_with_object({}) do |(sym, p), book|
      t = Ticker.find_by!(exchange: alpaca, base_asset: assets.fetch(sym))
      book[[t.id, :ask]] = BigDecimal(p['ask'])
      book[[t.id, :last]] = BigDecimal(p['last'])
    end
    r = bot.send(:get_orders_data, BigDecimal(c[:x]))
    { 'weights' => c[:weights], 'pairs' => pairs_of.(c[:weights]), 'rows' => c[:rows], 'prices' => c[:prices], 'limit' => c[:limit], 'x' => c[:x],
      'orders' => r.success? ? r.data.map { |o| [o[:ticker].base, o[:price].to_d.to_s('F'), o[:amount].to_d.to_s('F'), o[:quote_amount].to_d.to_s('F')] } : nil,
      'failure' => r.failure? ? r.errors.to_sentence : nil }
  end
end

# Bot::QuoteAmountLimitable#quote_amount_available_before_limit_reached and
# #quote_amount_limit_reached? as Ruby computes them. The closed and waiting buckets pluck decimal columns (BigDecimal); the
# stopped bucket plucks Arel.sql('COALESCE(quote_amount_exec, 0)'), which SQLite answers as its own INTEGER or REAL (Integer
# or Float in Ruby); each bucket is summed by Array#sum, the buckets added in that order, and the limit is the settings
# JSON's Integer or Float. Recorded with the class of the result: a Float leaks into the remainder (cap 60.03, a sole
# cancelled fill of 60.02: 0.00999999999999801, under the 0.01 floor).
cap_row = lambda do |kind, q|
  vector_row_id += 1
  row = { 'asset' => 'VBTC', 'status' => 0, 'external_id' => "rust-vector-#{vector_row_id}", 'order_type' => 0, 'price' => '64000',
          'created_at' => '2026-08-20 10:00:00' }
  case kind
  when 'closed' then row.merge('external_status' => 2, 'quote_amount' => q, 'quote_amount_exec' => q, 'amount_exec' => '0.001')
  when 'unknown' then row.merge('external_status' => 0, 'quote_amount' => q)
  when 'cancelled' then row.merge('external_status' => 3, 'quote_amount' => '100', 'quote_amount_exec' => q, 'amount_exec' => '0.001')
  when 'abandoned' then row.merge('external_status' => 4, 'quote_amount' => '100')
  end
end
cap_fixed = [
  [60.03, [%w[cancelled 60.02]]],                        # a Float remainder under the floor: reached
  [60.03, [%w[closed 60.02]]],                           # the same in BigDecimal: exactly 0.01, not reached
  [60.03, [%w[closed 30.01], %w[cancelled 30.01]]],      # BigDecimal + Float
  [60.03, [%w[cancelled 30.01], %w[cancelled 30.01]]],   # two Floats, Kahan-summed
  [100, [%w[cancelled 99.99]]],                          # Integer - Float
  [100, [%w[cancelled 99.995]]],
  [100, [%w[closed 99.995]]],
  [60.03, [['abandoned', nil], %w[cancelled 60.02]]],    # Integer 0, then a Float
  [60.03, [%w[cancelled 60], %w[cancelled 0.02]]],       # an integral fill is stored INTEGER, then a Float
  [1000, []],
  [50.5, [%w[unknown 25.25], %w[cancelled 25.24]]],
  [0.3, [%w[cancelled 0.1], %w[cancelled 0.2]]],
  [0.31, [%w[cancelled 0.1], %w[cancelled 0.2]]],
  [120.07, [%w[closed 40.02], %w[unknown 40.02], %w[cancelled 40.02]]]
]
cap_rng = Random.new(2_202_670)
cap_random = Array.new(26) do
  cap = [60.03, 100, 99.99, 120.07, 0.3, 75.5].sample(random: cap_rng)
  rows = Array.new(cap_rng.rand(1..4)) do
    kind = %w[closed unknown cancelled cancelled abandoned].sample(random: cap_rng)
    [kind, kind == 'abandoned' ? nil : format('%.2f', cap_rng.rand(1..3000) / 100.0)]
  end
  [cap, rows]
end
vectors['amount_caps'] = (cap_fixed + cap_random).map do |cap, specs|
  rows = specs.map { |kind, q| cap_row.(kind, q) }
  with_basket({ 'VBTC' => 1.0 }, settings: { 'quote_amount_limited' => true, 'quote_amount_limit' => cap }, rows:) do |bot, _assets, _alpaca|
    bot.update_columns(transient_data: bot.transient_data.merge('quote_amount_limit_enabled_at' => '2026-08-01T00:00:00.000Z'))
    bot = Bot.find(bot.id)
    left = bot.quote_amount_available_before_limit_reached
    value = case left
            when Float then { 'class' => 'Float', 'f' => [left].pack('G').unpack1('H*') }
            when BigDecimal then { 'class' => 'BigDecimal', 'd' => left.to_s('F') }
            else { 'class' => left.class.name, 'i' => left.to_s }
            end
    { 'weights' => { 'VBTC' => 1.0 }, 'pairs' => pairs_of.({ 'VBTC' => 1.0 }), 'rows' => rows, 'cap' => cap, 'available' => value,
      'reached' => bot.quote_amount_limit_reached? }
  end
end

# Bot::Lifecycle#start(start_fresh: false) on a stopped one-asset basket (daily, 60, started 2026-09-01 10:00), called at
# 2026-09-03 15:00 UTC: whether Rails runs it now, at the next checkpoint or at a delayed first run, read from the
# Bot::ActionJob it enqueues. The engine makes the same decision when the web asks it to continue a bot.
continue_row = lambda do |n, quote_exec, created_at|
  { 'asset' => 'VBTC', 'status' => 0, 'side' => 0, 'transaction_type' => 'REGULAR', 'external_id' => "rust-continue-#{n}", 'order_type' => 0,
    'price' => '64000', 'external_status' => 2, 'quote_amount' => '60', 'quote_amount_exec' => quote_exec, 'amount_exec' => '0.0009375',
    'created_at' => created_at }
end
stamped = '2026-09-03T10:00:00.500Z' # today's tick stamped last_action_job_at
two_days = [['60', '2026-09-01 10:00:01'], ['60', '2026-09-02 10:00:01']]
continue_cases = [
  ['owes_its_contribution', stamped, two_days, {}],                         # stamped, never placed: 60 owed, not under 60
  ['nothing_owed', stamped, two_days + [['60', '2026-09-03 10:00:01']], {}],
  ['a_cent_short_of_a_contribution', stamped, two_days + [['0.01', '2026-09-03 10:00:01']], {}],
  ['never_ticked', nil, [], {}],                                            # not restarting
  ['the_cap_leaves_less_than_a_contribution', stamped, [['60', '2026-09-01 10:00:01']],
   { 'quote_amount_limited' => true, 'quote_amount_limit' => 100 }],        # 120 owed, 40 left under the cap
  ['a_smart_interval_compares_with_its_split_amount', stamped, two_days,
   { 'smart_intervaled' => true, 'smart_interval_quote_amount' => 20.0 }], # 7 eight-hour intervals: 20 owed, not under 20
  ['a_future_start_time_is_not_a_delayed_first_run', stamped, two_days,
   { 'start_time_enabled' => true, 'start_time_mode' => 'date', 'start_at' => '2026-10-01T00:00:00Z' }]
]
adapter = ActiveJob::Base.queue_adapter
ActiveJob::Base.queue_adapter = :test
vectors['continue_start'] = continue_cases.each_with_index.map do |(name, last_action_job_at, specs, settings), i|
  rows = specs.each_with_index.map { |(q, t), n| continue_row.((i * 10) + n, q, t) }
  with_basket({ 'VBTC' => 1.0 }, settings:, rows:) do |bot, _assets, _alpaca|
    transient = bot.transient_data.merge('last_action_job_at' => last_action_job_at, 'quote_amount_limit_enabled_at' => '2026-08-01T00:00:00.000Z').compact
    bot.update_columns(status: Bot.statuses[:stopped], started_at: Time.utc(2026, 9, 1, 10), settings_changed_at: nil, transient_data: transient)
    bot = Bot.find(bot.id)
    now = Time.utc(2026, 9, 3, 15)
    travel_to(now, with_usec: true) do
      ActiveJob::Base.queue_adapter.enqueued_jobs.clear
      raise "#{name}: Rails refused the start: #{bot.errors.full_messages}" unless bot.start(start_fresh: false)

      jobs = ActiveJob::Base.queue_adapter.enqueued_jobs.select { |j| j[:job] == Bot::ActionJob }
      raise "#{name}: #{jobs.size} action jobs" unless jobs.size == 1

      at = jobs.first[:at] && Time.zone.at(jobs.first[:at]).utc
      decision = if at.nil? then 'now'
                 elsif at.round(6) == bot.next_interval_checkpoint_at.round(6) then 'checkpoint'
                 else "at #{at.iso8601(6)}"
                 end
      { 'name' => name, 'settings' => settings, 'last_action_job_at' => last_action_job_at, 'rows' => rows, 'now' => now.iso8601(6),
        'decision' => decision }
    end
  end
end
ActiveJob::Base.queue_adapter = adapter

# The web UI (rust/src/web). Everything below is what Rails itself answers, so the Rust port is held to it.
helpers = ApplicationController.helpers
shown = ->(value) { ERB::Util.html_escape(value).to_s } # what a view prints: escaped unless html_safe
i18n_calls = [
  ['en', 'devise.sessions.new.title', {}], ['de', 'devise.sessions.new.title', {}],
  ['en', 'links.api', {}], ['de', 'links.api', {}], # English only: the German page falls back; `&` is escaped
  ['en', 'bot.add_api_keys', { 'exchange' => %q(A<b>&"') }], # a plain key: the whole text is escaped
  ['en', 'ads.dca_profit_html', { 'years' => '<4>', 'profit' => '12', 'sp500_diff' => 'a&b' }], # an HTML key: only the arguments are
  ['en', 'devise.failure.invalid', { 'authentication_keys' => 'email' }],
  ['en', 'devise.failure.locked', {}], ['de', 'devise.failure.locked', {}], # from the Devise gem, English only
  ['en', 'devise.sessions.two_factor.title', {}], ['de', 'devise.sessions.two_factor.title', {}], # missing everywhere
  ['en', 'nowhere.some_key_html', {}], ['en', 'nowhere.user_id', { 'name' => 'a<b' }], ['en', 'nowhere._odd__key', { 'count' => 3 }]
] + %w[en pl ru].product([0, 1, 2, 4, 5, 11, 12, 21, 22, 24, 25, 101, 112]).flat_map { |locale, n| %w[days_left errors.messages.too_short].map { |key| [locale, key, { 'count' => n }] } }
vectors['i18n'] = {
  'locales' => I18n.available_locales.map(&:to_s),
  'default' => I18n.default_locale.to_s,
  'calls' => i18n_calls.map do |locale, key, args|
    I18n.with_locale(locale) do
      { 'locale' => locale, 'key' => key, 'args' => args,
        'view' => shown.(helpers.t(key, **args.symbolize_keys)), 'text' => I18n.t(key, **args.symbolize_keys) }
    end
  end,
  'escape' => [%q(a<b>&"'c), 'plain', 'ż & ☃'].map { |s| [s, shown.(s)] }
}
ts = helpers.turbo_stream
vectors['turbo'] = {
  'replace' => ts.replace('bot_1', '<p>a &amp; b</p>'.html_safe), 'update' => ts.update('bot_1', '<p>x</p>'.html_safe),
  'append' => ts.append('orders', '<tr></tr>'.html_safe), 'prepend' => ts.prepend('flash', '<div>hi</div>'.html_safe),
  'remove' => ts.remove('bot_1'), 'refresh' => ts.refresh(request_id: nil),
  'redirect' => ts.action(:redirect, '/de/bots?a=1&b=2'), # SharedHelper#turbo_stream_redirect
  # Bot#broadcast_columns_lock_update, as Turbo::StreamsChannel.broadcast_action_to renders it.
  'add_class' => helpers.turbo_stream_action_tag(:add_class, target: 'columns_bot_1', template: nil, 'class-name': 'bot-locked'),
  'remove_class' => helpers.turbo_stream_action_tag(:remove_class, target: 'columns_bot_1', template: nil, 'class-name': 'bot-locked'),
  'stream_from' => helpers.turbo_stream_from('user_7', :bot_updates),
  'stream_name' => Turbo::StreamsChannel.verified_stream_name(Turbo::StreamsChannel.signed_stream_name(['user_7', :bot_updates])),
  'content_type' => Mime[:turbo_stream].to_s
}
remote_ip = lambda do |remote_addr, forwarded_for, client_ip|
  env = { 'REMOTE_ADDR' => remote_addr, 'HTTP_X_FORWARDED_FOR' => forwarded_for, 'HTTP_CLIENT_IP' => client_ip }.compact
  ActionDispatch::RemoteIp::GetIp.new(ActionDispatch::Request.new(env), false, ActionDispatch::RemoteIp::TRUSTED_PROXIES).to_s
end
trusted_proxy = ->(addr) { ActionDispatch::RemoteIp::TRUSTED_PROXIES.any? { |proxy| proxy === addr } }
vectors['remote_ip'] = [
  # a peer that is a trusted proxy: one hop, a private hop behind it, several hops, nothing but private hops
  ['10.0.0.5', '198.51.100.7', nil], ['10.0.0.5', '198.51.100.7, 10.0.0.9', nil], ['10.0.0.5', '1.1.1.1, 198.51.100.7', nil],
  ['10.0.0.5', '203.0.113.50, 198.51.100.7, 10.0.0.9, 192.168.1.4', nil], ['10.0.0.5', '10.1.1.1, 192.168.1.1', nil],
  ['172.16.0.1', '172.32.0.1', nil], ['169.254.1.1', 'fe80::1', nil],
  # malformed entries
  ['10.0.0.5', 'not-an-ip, 198.51.100.7', nil], ['10.0.0.5', '198.51.100.7, unknown, , 10.0.0.9', nil], ['10.0.0.5', '198.51.100.7/8', nil],
  ['10.0.0.5', '', nil],
  # entries with a port, as a proxy may write its peer, also behind an entry only the caller wrote
  ['10.0.0.5', '203.0.113.7:54321', nil], ['10.0.0.5', '[2001:db8::7]:54321', nil], ['10.0.0.5', '[2001:db8::7]', nil],
  ['10.0.0.5', '198.51.100.99, 203.0.113.7:54321', nil], ['10.0.0.5', '198.51.100.99, [2001:db8::7]:54321', nil],
  ['10.0.0.5', '198.51.100.99, 203.0.113.7:54321, 10.0.0.9:443', nil], ['10.0.0.5', '198.51.100.99 203.0.113.7', nil],
  # entries Rails cannot read, behind an entry only the caller wrote; IPv4-mapped addresses
  ['10.0.0.5', '198.51.100.99, garbage, 10.0.0.9', nil], ['10.0.0.5', '198.51.100.99, 203.0.113.7:notaport', nil],
  ['10.0.0.5', '198.51.100.99, 203.0.113.7/32', nil], ['10.0.0.5', '198.51.100.99, ::ffff:10.0.0.9', nil],
  ['10.0.0.5', '198.51.100.99, ::ffff:203.0.113.7', nil], ['10.0.0.5', '198.51.100.99, [::ffff:203.0.113.7]:443', nil],
  # Client-Ip
  ['127.0.0.1', nil, '198.51.100.8'], ['127.0.0.1', '198.51.100.7', '198.51.100.8'], ['10.0.0.5', nil, '198.51.100.8:1234'],
  ['10.0.0.5', nil, '198.51.100.9, 198.51.100.8'],
  # IPv6
  ['10.0.0.5', '2001:db8::1', nil], ['::1', 'fd00::1, 2001:db8::2', nil], ['fd00::5', '2001:db8::7', nil],
  # a peer that is not a trusted proxy, with and without headers of its own making
  ['203.0.113.9', nil, nil], ['203.0.113.9', '198.51.100.7', nil], ['203.0.113.9', '198.51.100.7, 10.0.0.9', nil],
  ['203.0.113.9', nil, '198.51.100.8'], ['2001:db8::9', '198.51.100.7', nil]
].map do |addr, forwarded, client|
  { 'remote_addr' => addr, 'forwarded_for' => forwarded, 'client_ip' => client, 'peer_trusted' => trusted_proxy.(addr),
    'ip' => remote_ip.(addr, forwarded, client) }
end
vectors['tracker'] = {
  'cash' => (Tracker::UnfundedCash::FIAT + Tracker::UnfundedCash::STABLECOINS).sort,
  # User#show_cash? for what the tracker_settings column can hold.
  'show_cash' => [nil, {}, { 'other' => true }, { 'show_cash' => true }, { 'show_cash' => false }, { 'show_cash' => nil },
                  { 'show_cash' => '' }, { 'show_cash' => ' ' }, { 'show_cash' => 'false' }, { 'show_cash' => 0 }, { 'show_cash' => [] },
                  { 'show_cash' => [1] }, { 'show_cash' => {} }].map do |settings|
    { 'column' => settings&.to_json, 'shown' => User.new(tracker_settings: settings).show_cash? }
  end
}
vectors['navbar'] = {
  'bot_count' => [0, 1, 9, 10, 99, 100, 999, 1000, 12_345].map do |count|
    size = helpers.send(:bot_count_font_size, count)
    { 'count' => count, 'font_size' => size.to_s, 'baseline' => helpers.send(:bot_count_baseline, size).to_s }
  end
}
vectors['rack_attack'] = {
  'throttles' => Rack::Attack.throttles.slice('users/login', 'users/verify_two_factor').transform_values { |t| { 'limit' => t.limit, 'period' => t.period } },
  'normalize' => ['/login', '/login/', '//login', '/de//login/', '/'].map { |p| [p, RackAttackPaths.normalize(p)] },
  'body' => "#{I18n.t('errors.throttled')}\n"
}
vectors['devise'] = { 'maximum_attempts' => Devise.maximum_attempts, 'unlock_in' => Devise.unlock_in.to_i,
                      'pending_ttl' => Users::SessionsController::PENDING_TTL.to_i, 'session_expire_after' => Rails.application.config.session_options[:expire_after].to_i }
# request.base_url, which the Origin header of a form POST must equal (valid_request_origin?), from the
# stack production runs: Puma builds the env (it derives rack.url_scheme from the forwarded headers),
# ActionDispatch::AssumeSSL sits in front of the app when config/environments/production.rb turns SSL
# on ('ssl' below: it sets assume_ssl and force_ssl from the one flag), and Rack and Action Dispatch
# read the result. Each request goes over a raw socket, so header lines arrive as written, repeats included.
require 'puma'
require 'puma/server'
require 'socket'
unless Rails.root.join('config/environments/production.rb').read.match?(/config\.assume_ssl = ssl_enabled\n\s*config\.force_ssl = ssl_enabled\n/)
  raise 'production.rb no longer sets assume_ssl and force_ssl from the one flag, which the base_url vectors assume'
end
base_url_app = ->(env) { [200, { 'content-type' => 'text/plain' }, [ActionDispatch::Request.new(env).base_url]] }
# [server, port] of `app` behind Puma on a local port.
over_puma = lambda do |app|
  server = Puma::Server.new(app, nil, log_writer: Puma::LogWriter.null)
  port = server.add_tcp_listener('127.0.0.1', 0).addr[1]
  server.run
  [server, port]
end
# The body of the answer to one GET with exactly these header lines.
ask_puma = lambda do |port, host, headers|
  answer = TCPSocket.open('127.0.0.1', port) do |socket|
    socket.write("GET / HTTP/1.1\r\nHost: #{host}\r\n#{headers.map { |line| "#{line}\r\n" }.join}Connection: close\r\n\r\n")
    socket.read
  end
  head, body = answer.split("\r\n\r\n", 2)
  raise "#{host} #{headers}: #{head}" unless head.start_with?('HTTP/1.1 200')

  body
end
base_url_servers = { false => base_url_app, true => ActionDispatch::AssumeSSL.new(base_url_app) }.transform_values(&over_puma)
base_url = ->(ssl, host, headers) { ask_puma.(base_url_servers.fetch(ssl).last, host, headers) }
forwarded_headers = [
  [], ['X-Forwarded-Proto: https'], ['X-Forwarded-Proto: http'], ['X-Forwarded-Proto: https,http'], ['X-Forwarded-Proto: http,https'],
  ['X-Forwarded-Proto: https, http'], ['X-Forwarded-Proto: http https'], ['X-Forwarded-Proto: https', 'X-Forwarded-Proto: http'],
  ['X-Forwarded-Proto: HTTPS'], ['X-Forwarded-Proto: ftp'], ['X-Forwarded-Proto: https,ftp'], ['X-Forwarded-Proto: httpsx'],
  ['X-Forwarded-Proto: wss'], ['X-Forwarded-Proto: ws'],
  ['X-Forwarded-Ssl: on'], ['X-Forwarded-Ssl: off'], ['X-Forwarded-Ssl: On'], ['X-Forwarded-Ssl: on', 'X-Forwarded-Proto: http'],
  ['X-Forwarded-Scheme: https'], ['X-Forwarded-Scheme: http'], ['X-Forwarded-Proto: http', 'X-Forwarded-Scheme: https'],
  ['X-Forwarded-Proto: ftp', 'X-Forwarded-Scheme: https'], ['X-Forwarded-Proto: https', 'X-Forwarded-Scheme: http'],
  ['X-Forwarded-Proto: ftp', 'X-Forwarded-Scheme: HTTPS'],
  ['Forwarded: proto=https'], ['Forwarded: proto=http', 'X-Forwarded-Proto: https'], ['Forwarded: proto=https', 'X-Forwarded-Proto: http'],
  ['Forwarded: for=192.0.2.1;proto=https, for=198.51.100.2;proto=http'], ['Forwarded: for=192.0.2.1;proto=http, for=198.51.100.2;proto=https'],
  ['Forwarded: Proto = "https"'], ['Forwarded: for="[2001:db8::1]:4711";proto=https;by=203.0.113.43'], ['Forwarded: proto="ht\\tps" ; for=x'],
  ['Forwarded: proto=https;secret=1'], ['Forwarded: proto=ftp', 'X-Forwarded-Proto: https'], ['Forwarded: for=192.0.2.1', 'X-Forwarded-Proto: https'],
  ['Forwarded: proto=http', 'X-Forwarded-Ssl: on'], ['Forwarded: proto="https'], ['Forwarded: proto=https', 'Forwarded: proto=http'],
  ['X-Forwarded-Host: public.example.org'], ['X-Forwarded-Host: public.example.org', 'X-Forwarded-Proto: https'],
  ['X-Forwarded-Host: public.example.org:8443', 'X-Forwarded-Proto: https'], ['X-Forwarded-Host: public.example.org:443', 'X-Forwarded-Proto: https'],
  ['X-Forwarded-Host: first.example, public.example.org'], ['X-Forwarded-Host: first.example,public.example.org:81'],
  ['X-Forwarded-Host: public.example.org,'], ['X-Forwarded-Host:'],
  ['X-Forwarded-Port: 8443', 'X-Forwarded-Proto: https'], ['Forwarded: host=public.example.org;proto=https'],
  # a header sent as several lines: Puma hands Rack one value
  ['X-Forwarded-Proto: http', 'X-Forwarded-Proto: https'], ['X-Forwarded-Scheme: https', 'X-Forwarded-Scheme: http'],
  ['X-Forwarded-Ssl: on', 'X-Forwarded-Ssl: on'], ['X-Forwarded-Host: first.example', 'X-Forwarded-Host: public.example.org'],
  ['X-Forwarded-Host: public.example.org', 'X-Forwarded-Proto: https', 'X-Forwarded-Host: last.example:8443']
]
base_url_cases = forwarded_headers.map { |headers| ['bot.example.com:8080', headers] } +
                 ['bot.example.com', 'bot.example.com:443', 'bot.example.com:80', '[::1]:3000', '[::1]'].product([[], ['X-Forwarded-Proto: https']])
vectors['base_url'] = [false, true].product(base_url_cases).map do |ssl, (host, headers)|
  { 'ssl' => ssl, 'host' => host, 'headers' => headers, 'base_url' => base_url.(ssl, host, headers) }
end
base_url_servers.each_value { |server, _| server.stop(true) }
# The client address when a forwarding header arrives as several lines (a proxy that appends its
# own line after whatever the caller sent): what Puma makes of the lines, and the address Rails
# then takes. The peer is 127.0.0.1, a trusted proxy.
remote_ip_server = over_puma.(lambda do |env|
  ip = ActionDispatch::RemoteIp::GetIp.new(ActionDispatch::Request.new(env), false, ActionDispatch::RemoteIp::TRUSTED_PROXIES).to_s
  [200, { 'content-type' => 'application/json' }, [{ 'remote_addr' => env['REMOTE_ADDR'], 'forwarded_for' => env['HTTP_X_FORWARDED_FOR'],
                                                     'client_ip' => env['HTTP_CLIENT_IP'], 'ip' => ip }.to_json]]
end)
vectors['remote_ip_lines'] = [
  ['X-Forwarded-For: 198.51.100.7'], ['X-Forwarded-For: 1.1.1.1', 'X-Forwarded-For: 198.51.100.7'],
  ['X-Forwarded-For: 1.1.1.1, 2.2.2.2', 'X-Forwarded-For: 198.51.100.7, 10.0.0.9'],
  ['X-Forwarded-For: 198.51.100.7', 'X-Forwarded-For: 10.0.0.9', 'X-Forwarded-For: 192.168.1.4'],
  ['X-Forwarded-For: 198.51.100.7', 'X-Forwarded-For: 1.1.1.1'],
  ['Client-Ip: 1.1.1.1', 'Client-Ip: 198.51.100.8'],
  ['X-Forwarded-For: 1.1.1.1', 'Client-Ip: 198.51.100.8', 'X-Forwarded-For: 198.51.100.7']
].map { |headers| { 'headers' => headers }.merge(JSON.parse(ask_puma.(remote_ip_server.last, 'bot.example.com', headers))) }
remote_ip_server.first.stop(true)
# users.time_zone holds one of these names; the crate embeds the table (src/web/time_zones.json).
time_zones = ActiveSupport::TimeZone::MAPPING
File.write(Rails.root.join('rust/src/web/time_zones.json'), "#{JSON.pretty_generate(time_zones)}\n")
File.write(ARGV.fetch(0), "#{JSON.pretty_generate(vectors)}\n")
puts "wrote #{ARGV.fetch(0)}"
