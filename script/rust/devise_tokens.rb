# Records rust/tests/fixtures/devise_token_vectors.json: Devise's reset_password_token digests under a fixture-only
# secret, built exactly as Devise builds its own generator in this app (same key generator class, digest, iterations).
#   RAILS_ENV=test bin/rails runner script/rust/devise_tokens.rb > rust/tests/fixtures/devise_token_vectors.json
require 'json'

live = Devise.token_generator
key_generator = live.instance_variable_get(:@key_generator).instance_variable_get(:@key_generator)
digest_class = key_generator.instance_variable_get(:@hash_digest_class)
iterations = key_generator.instance_variable_get(:@iterations)
build = lambda do |secret|
  Devise::TokenGenerator.new(ActiveSupport::CachingKeyGenerator.new(ActiveSupport::KeyGenerator.new(secret, iterations:, hash_digest_class: digest_class)))
end
# The reconstruction is Devise's own generator: same answer under the app's secret.
raise 'reconstruction differs from Devise.token_generator' unless build.call(Devise.secret_key).digest(User, :reset_password_token, 'probe') ==
                                                                live.digest(User, :reset_password_token, 'probe')

secret = 'a16-fixture-secret-not-for-any-install'
generator = build.call(secret)
raws = ['seededResetToken0001', 'xz-Q_9aBcDeFgHiJkLmN', 'Ünïcode token', 'a']
puts JSON.pretty_generate('secret_key_base' => secret, 'key_digest' => digest_class.name, 'iterations' => iterations,
                          'cases' => raws.map { |raw| { 'raw' => raw, 'digest' => generator.digest(User, :reset_password_token, raw) } },
                          'blank_digest' => generator.digest(User, :reset_password_token, ''))
