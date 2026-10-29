# Production boot in a private directory. Only container paths are substituted; the task,
# initializers, migrations, encryption and Solid Queue adapter run as shipped.
require_relative '../../lib/export_scratch'
require 'fileutils'

mode, directory = ARGV
storage = File.realpath(File.join(directory, 'storage'))
{ STORAGE: storage, OUTPUT: File.join(directory, 'export'), CONTAINER_MARKER: File.join(directory, '.dockerenv') }.each do |name, value|
  ExportScratch.send(:remove_const, name)
  ExportScratch.const_set(name, value)
end
unless mode == 'restore'
  ENV['EXPORT_ENTRYPOINT'] = '1'
  ExportScratch.validate!
end

require_relative '../../config/application'
require 'webmock'
WebMock.enable!
WebMock.disable_net_connect!
database_dir = mode == 'restore' ? File.join(directory, 'export/portable') : storage
database_config = { 'production' => %w[primary queue cache cable].to_h do |name|
  suffix = name == 'primary' ? '' : "_#{name}"
  [name, { 'adapter' => 'sqlite3', 'database' => File.join(database_dir, "production#{suffix}.sqlite3"),
           'migrations_paths' => name == 'primary' ? 'db/migrate' : "db/#{name}_migrate" }]
end }
config_path = File.join(directory, 'database.yml')
File.write(config_path, database_config.to_yaml)
Rails.application.config.paths['config/database'] = config_path
Rails.application.load_tasks

case mode
when 'export'
  Rake::Task['export:portable'].invoke
when 'seed'
  Rake::Task['db:schema:load:primary'].invoke
  now = Time.current
  secret = 'planted-export-credential-123456789'
  user = User.new(password: 'kept-password')
  User.insert_all!([{ id: 1, email: 'export@example.test', encrypted_password: user.encrypted_password,
                      otp_secret_key: 'KEPTOTPSECRET', otp_module: 1, reset_password_token: secret,
                      confirmation_token: secret, remember_created_at: now }])
  Exchange.insert_all!([{ id: 1, name: 'Binance', type: 'Exchanges::Binance' }])
  attributes = EncryptedCredentials::COVERAGE.fetch('ApiKey').fetch(:attributes).index_with { secret }
  ApiKey.insert_all!([attributes.merge(id: 1, user_id: 1, exchange_id: 1, status: 1)])
  FeeApiKey.insert_all!([{ exchange_id: 1, key: secret, secret: secret, passphrase: secret }])
  AppConfig.insert_all!(Export::Portable::APP_CONFIG_KEYS.map { |key| { key: key, value: secret } } +
                       [{ key: 'proxy_binance', value: secret }, { key: 'smtp_future_option', value: secret },
                        { key: 'registration_open', value: 'true' }])
  Bot.insert_all!([{ id: 1, user_id: 1, exchange_id: 1, type: 'Bots::Signal', status: 1, label: '../../bad / label' }])
  BotSignal.insert_all!([{ bot_id: 1, amount: 10, token: secret, enabled: true }])
  Rules::Withdrawal.insert_all!([{ id: 1, user_id: 1, exchange_id: 1, type: 'Rules::Withdrawal', status: 1,
                                   address: 'kept-withdrawal-address' }])
  AccountTransaction.insert_all!([{ user_id: 1, exchange_id: 1, api_key_id: 1, entry_type: 0, base_currency: 'BTC',
                                    base_amount: 1, tx_id: 'tracker-id', description: '=formula description', transacted_at: now }])
  Transaction.insert_all!(%w[closed open].map do |status|
    { bot_id: 1, exchange_id: 1, external_id: "#{status}-order", status: :submitted, external_status: status,
      order_type: :market_order, side: :buy, amount: 1, price: 2, base: 'BTC', quote: 'USD' }
  end)
  connection = ApplicationRecord.connection
  insert = lambda do |table, fields|
    fields = { created_at: now, updated_at: now }.merge(fields).stringify_keys
    fields.slice!(*connection.columns(table).map(&:name))
    connection.execute("INSERT INTO #{table} (#{fields.keys.join(',')}) VALUES (#{fields.values.map { |value| connection.quote(value) }.join(',')})")
  end
  insert.call('oauth_applications', id: 1, name: 'client', uid: 'client-id', secret: secret, registration_access_token: secret)
  insert.call('oauth_access_tokens', application_id: 1, token: secret, refresh_token: secret)
  insert.call('oauth_access_grants', application_id: 1, resource_owner_id: 1, token: secret, expires_in: 600, redirect_uri: 'https://example.test')
  insert.call('connected_clients', user_id: 1, oauth_application_id: 1, mcp_tools: "[\"#{secret}\"]")
  insert.call('action_mcp_sessions', id: secret, session_data: "{\"token\":\"#{secret}\"}")
  insert.call('action_mcp_session_messages', session_id: secret, message_type: 'request', message_json: "{\"token\":\"#{secret}\"}")
  insert.call('action_mcp_session_subscriptions', session_id: secret, uri: secret)
  insert.call('action_mcp_session_tasks', id: secret, session_id: secret, last_updated_at: now, request_params: "{\"token\":\"#{secret}\"}")

  removed = [secret]
  %w[api_keys fee_api_keys app_configs].each do |table|
    columns = table == 'app_configs' ? ['value'] : EncryptedCredentials::COVERAGE.fetch(table.classify).fetch(:attributes)
    rows = connection.select_rows("SELECT #{columns.join(',')} FROM #{table}#{" WHERE key != 'registration_open'" if table == 'app_configs'}")
    removed.concat(rows.flatten.compact)
  end
  AppConfig.set(EngineLease::KEY, { engine: 'rust', version: '0.1.0', since: now.iso8601 }.to_json)
  # Reconstruct the September schema. The real migrations must recreate its missing
  # columns and table, update the indexes and enqueue the asset resolution job offline.
  connection.remove_column(:account_transactions, :base_asset_id)
  connection.remove_column(:transactions, :run_identity)
  connection.drop_table(:portfolio_venue_snapshots)
  connection.remove_index(:transactions, column: %i[exchange_id external_id])
  connection.add_index(:transactions, :external_id, unique: true)
  connection.add_index(:tickers, %i[exchange_id base quote], unique: true, name: 'index_exchange_tickers_on_unique_base_and_quote')
  connection.change_column_null(:connected_clients, :mcp_tools, false)
  connection.change_column_null(:connected_clients, :rest_tools, false)
  connection.execute("DELETE FROM schema_migrations WHERE version >= '20260921180000'")
  %w[production_queue.sqlite3 production_cache.sqlite3 production_cable.sqlite3 backup.sqlite3 unknown.log].each do |file|
    File.write(File.join(storage, file), secret)
  end
  File.write(File.join(storage, '.secrets'), "SECRET_KEY_BASE=stale-secret\nSMTP_PASSWORD=#{secret}\n")
  puts({ removed: removed.uniq }.to_json)
when 'restore'
  Rake::Task['environment'].invoke
  user = User.find(1)
  api_key = ApiKey.find(1)
  puts({ otp: user.otp_secret_key, address: Rules::Withdrawal.find(1).address,
         password_valid: user.valid_password?('kept-password'), bots: Bot.pluck(:status).map { |status| Bot.statuses.fetch(status) },
         rules: Rule.pluck(:status).map { |status| Rule.statuses.fetch(status) }, signals: BotSignal.pluck(:enabled),
         api_key_empty: api_key.incorrect? && EncryptedCredentials::COVERAGE.fetch('ApiKey').fetch(:attributes).all? { |key| api_key[key].nil? },
         users_cleared: user.reset_password_token.nil? && user.confirmation_token.nil? && user.remember_created_at.nil?,
         sessions_deleted: Export::Portable::SESSION_TABLES.all? do |table|
           ApplicationRecord.connection.select_value("SELECT COUNT(*) FROM #{table}").zero?
         end,
         fee_keys_deleted: FeeApiKey.count.zero?,
         engine_lease_cleared: !AppConfig.exists?(key: EngineLease::KEY),
         jobs: SolidQueue::Job.pluck(:class_name), ready_jobs: SolidQueue::ReadyExecution.count,
         cache_empty: SolidCache::Entry.count.zero?, cable_empty: SolidCable::Message.count.zero?,
         registration_open: AppConfig.get('registration_open') }.to_json)
end
