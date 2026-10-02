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
