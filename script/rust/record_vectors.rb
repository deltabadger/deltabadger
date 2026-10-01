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
  'gems' => %w[activerecord bcrypt bigdecimal rotp].to_h { |g| [g, Gem.loaded_specs.fetch(g).version.to_s] }
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
                    'EAPI:Invalid nonce', 'qty must be >= 0.000027']
vectors['failure_kinds'] = { 'Exchanges::Alpaca' => Exchanges::Alpaca.new, 'Exchanges::Kraken' => Exchanges::Kraken.new }.flat_map do |type, ex|
  failure_messages.map { |m| [type, m, ex.failure_kind([m])&.to_s, ex.transient_error?([m]), ex.throttled_error?([m])] }
end
# The Ruby the Alpaca port mirrors; rust/tests/venue_rules.rs fails when any of it changes, until re-recorded and re-checked.
vectors['ported_sources'] = %w[app/models/clients/alpaca.rb app/models/exchanges/alpaca.rb app/models/client.rb]
                              .to_h { |f| [f, Digest::SHA256.file(Rails.root.join(f)).hexdigest] }
File.write(ARGV.fetch(0), "#{JSON.pretty_generate(vectors)}\n")
puts "wrote #{ARGV.fetch(0)}"
