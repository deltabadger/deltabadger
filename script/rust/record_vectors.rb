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
  'otp_seed' => [User, :otp_secret_key, 'JBSWY3DPEHPK3PXPJBSWY3DP'],
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
  'enums' => {
    'bot_status' => Bot.statuses, 'transaction_status' => Transaction.statuses, 'transaction_side' => Transaction.sides,
    'transaction_order_type' => Transaction.order_types, 'transaction_external_status' => Transaction.external_statuses,
    'api_key_status' => ApiKey.statuses, 'api_key_key_type' => ApiKey.key_types, 'user_otp_module' => User.otp_modules
  },
  'gems' => %w[activerecord bcrypt bigdecimal rotp].to_h { |g| [g, Gem.loaded_specs.fetch(g).version.to_s] }
}
File.write(ARGV.fetch(0), "#{JSON.pretty_generate(vectors)}\n")
puts "wrote #{ARGV.fetch(0)}"
