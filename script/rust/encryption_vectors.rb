# Records rust/tests/fixtures/encryption_vectors.json (PLAN-A29 r2.7): Rails' deltabadger:encryption tasks run on one
# install holding every credential kind, with what they printed and every credential table before and after. The Rust
# commands are run on the same rows (rust/tests/rotation.rs) and must differ only by PLAN-A29's listed rewrites.
#   RAILS_ENV=test bin/rails runner script/rust/encryption_vectors.rb <out.json>     (a freshly loaded schema)
# Rows are written with insert_all (each encrypted attribute encrypts its value as it is written), so no callback runs.
require 'json'
require 'digest'
require 'stringio'

SOURCES = %w[lib/tasks/encryption.rake app/services/encrypted_credentials.rb app/services/encrypted_credentials/report.rb
             app/services/encrypted_credentials/reset.rb config/initializers/secret_key_base_guard.rb
             config/initializers/active_record_encryption.rb config/initializers/encryption_key_check.rb].freeze
TABLES = %w[users exchanges assets api_keys fee_api_keys app_configs rules oauth_applications oauth_access_tokens
            oauth_access_grants connected_clients bots account_transactions].freeze
out = ARGV.fetch(0)
Rails.application.load_tasks
t = '2026-01-01 00:00:00'

User.insert_all!([{ id: 1, email: 'o@example.com', encrypted_password: 'x', name: 'Owner', admin: true, otp_module: 1,
                    otp_secret_key: 'JBSWY3DPEHPK3PXP', created_at: t, updated_at: t }])
Exchange.insert_all!([{ id: 1, type: 'Exchanges::Kraken', name: 'Kraken', maker_fee: '0.25', taker_fee: '0.4', created_at: t, updated_at: t }])
Asset.insert_all!([{ id: 1, external_id: 'bitcoin', symbol: 'BTC', name: 'Bitcoin', category: 'Cryptocurrency', created_at: t, updated_at: t },
                   { id: 2, external_id: 'EUR.FOREX', symbol: 'EUR', name: 'Euro', category: 'Currency', created_at: t, updated_at: t }])
ApiKey.insert_all!([{ id: 1, user_id: 1, exchange_id: 1, key_type: 0, status: 1, key: 'test-key',
                      secret: 'test-secret', passphrase: nil, created_at: t, updated_at: t },
                    { id: 2, user_id: 1, exchange_id: 1, key_type: 1, status: 1, key: 'w-key',
                      secret: 'w-secret', passphrase: 'w-pass', created_at: t, updated_at: t }])
FeeApiKey.insert_all!([{ id: 1, exchange_id: 1, key: 'fee-key', secret: 'fee-secret' }])
AppConfig.insert_all!(%w[smtp_password coingecko_api_key market_data_token registration_open].each_with_index.map do |key, i|
  { id: i + 1, key: key, value: "value-#{i}", created_at: t, updated_at: t }
end)
Rule.insert_all!([{ id: 1, type: 'Rules::Withdrawal', user_id: 1, exchange_id: 1, asset_id: 1, status: 1, settings: {},
                    address: 'bc1qexampleaddress', created_at: t, updated_at: t }])
Doorkeeper::Application.insert_all!([{ id: 1, name: 'MCP', uid: 'uid-1', redirect_uri: 'http://localhost/cb', created_at: t, updated_at: t }])
Doorkeeper::AccessToken.insert_all!([{ id: 1, application_id: 1, resource_owner_id: 1, token: 'bearer-1', created_at: t }])
Doorkeeper::AccessGrant.insert_all!([{ id: 1, application_id: 1, resource_owner_id: 1, token: 'grant-1', expires_in: 600,
                                       redirect_uri: 'http://localhost/cb', created_at: t }])
ConnectedClient.insert_all!([{ id: 1, user_id: 1, oauth_application_id: 1, created_at: t, updated_at: t }])
Bot.insert_all!([{ id: 1, type: 'Bots::DcaMultiAsset', status: 1, exchange_id: 1, user_id: 1, started_at: t,
                   settings: { 'interval' => 'week', 'quote_amount' => 10.0, 'quote_asset_id' => 2, 'allocations' => { '1' => 1.0 } },
                   transient_data: {}, created_at: t, updated_at: t }])
AccountTransaction.insert_all!([{ id: 1, user_id: 1, exchange_id: 1, api_key_id: 1, entry_type: 0, base_amount: '1', base_currency: 'BTC',
                                  transacted_at: t, created_at: t, updated_at: t }])

dump = -> { TABLES.to_h { |table| [table, ActiveRecord::Base.connection.select_all("SELECT * FROM #{table} ORDER BY id").to_a] } }
run = lambda do |task, env|
  env.each { |k, v| ENV[k] = v }
  stdout = StringIO.new
  stderr = StringIO.new
  status = 0
  $stdout = stdout
  $stderr = stderr
  begin
    Rake::Task[task].reenable
    Rake::Task[task].invoke
  rescue SystemExit => e
    status = e.status
  ensure
    $stdout = STDOUT
    $stderr = STDERR
    env.each_key { |k| ENV.delete(k) }
  end
  { 'status' => status, 'stdout' => stdout.string, 'stderr' => stderr.string }
end

before = dump.call
report = run.call('deltabadger:encryption:report', {})
unconfirmed = run.call('deltabadger:encryption:reset', {})
raise 'the unconfirmed reset changed something' unless dump.call == before

reset = run.call('deltabadger:encryption:reset', 'CONFIRM' => 'clear-credentials')
File.write(out, JSON.pretty_generate(
  'sources' => SOURCES.to_h { |path| [path, Digest::SHA256.file(Rails.root.join(path)).hexdigest] },
  'secret_key_base' => Rails.application.secret_key_base,
  'independent' => EncryptionKeys.independent?,
  'before' => before, 'after' => dump.call,
  'report' => report, 'unconfirmed' => unconfirmed, 'reset' => reset
) + "\n")
puts "recorded #{out}"
