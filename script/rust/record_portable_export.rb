# The Rails half of `deltabadger export`'s parity (rust/tests/export.rs is the Rust half, PLAN-B4b §9):
#   bin/rails runner script/rust/record_portable_export.rb rust/tests/fixtures/portable_export.json
# One seeded install (several users and zones, every entry type, ties on transacted_at, bots of several kinds incl.
# deleted and archived, labels with accents, ß, CJK, emoji, 150 characters and `../`), exported by Rails' own
# `export:portable` through test/support/portable_export_process.rb as test/integration/portable_export_test.rb runs it
# (a production boot in a private directory, the entrypoint's load_secrets wrapper). Recorded: the seed's rows (the Rust
# test installs the same input), Rails' scrubbed rows, tracker.csv, every bot CSV's name and bytes, README_CSV.txt and
# the manifest (versions replaced by placeholders), Rails' scrub lists, and I18n's default transliteration table.
# Then Rails' own `seed` mode (its September-schema install, brought forward by the real migrations) is exported too,
# and its primary is kept gzipped next to the fixture for restore 5.2.
require 'json'
require 'digest'
require 'open3'
require 'tmpdir'
require 'zlib'
require 'sqlite3'
require_relative 'schema_template'

module PortableExport
  module_function

  PORTED = %w[app/services/export/portable.rb lib/export_scratch.rb app/models/account_transaction.rb
              app/models/bot/exportable.rb app/models/concerns/csv_safe.rb test/support/portable_export_process.rb
              lib/tasks/export.rake].freeze
  NOW = '2026-10-01 21:30:00'.freeze
  NOW6 = '2026-10-01 21:30:00.000000'.freeze
  SECRET_KEY_BASE = 'portable-export-recorder-secret-' * 4
  # A bcrypt hash of 'kept-password' (cost 4), fixed so a re-record writes the same bytes.
  PASSWORD = '$2a$04$7zQUoeeoSwshAMNiK0uH/.hnkPMcye4soRlbE9necCunOi2FaRG6q'.freeze
  PLANTED = 'planted-export-credential-'.freeze
  ZONES = ['UTC', 'Berlin', 'America/New_York', 'Asia/Kolkata', 'Tokyo'].freeze
  LABELS = ['Café Crème', 'Straße nach Ålesund', '比特币 定投', '🚀 To the moon 🚀', 'A' * 150, '../../etc/passwd',
            nil, '', '   ', 'Æon ŒUVRE ø', 'Bot_with_underscores', 'UPPER lower 123', '--dash--', 'Ünïcödé ñ', 'Ĳssel ŀl'].freeze

  def insert(db, table, fields)
    db.execute("INSERT INTO #{table} (#{fields.keys.join(',')}) VALUES (#{(['?'] * fields.size).join(',')})", fields.values)
  end

  def seed(path)
    SchemaTemplate.load_primary(path)
    db = ActiveRecord::Base.connection.raw_connection
    t = { created_at: NOW, updated_at: NOW }
    t6 = { created_at: NOW6, updated_at: NOW6 }
    users = ZONES.each_with_index.map do |zone, i|
      insert(db, 'users', { id: i + 1, email: "owner#{i + 1}@example.test", name: "Owner #{i + 1}", encrypted_password: PASSWORD,
                            time_zone: zone, otp_secret_key: i.even? ? "KEPTOTPSECRET#{i}" : nil, otp_module: i.even? ? 1 : 0,
                            reset_password_token: "#{PLANTED}reset-#{i}", confirmation_token: "#{PLANTED}confirm-#{i}",
                            remember_created_at: NOW, confirmed_at: NOW, admin: i.zero? ? 1 : 0 }.merge(t))
      i + 1
    end
    venues = [['Exchanges::Kraken', 'Kraken'], ['Exchanges::Binance', 'Binance'], ['Exchanges::Coinbase', 'Coinbase'],
              ['Exchanges::Alpaca', 'Alpaca']].each_with_index.map do |(type, name), i|
      insert(db, 'exchanges', { id: i + 1, type: type, name: name, maker_fee: '0.1', taker_fee: '0.2' }.merge(t))
      i + 1
    end
    # Every encrypted ApiKey column planted, every key type.
    users.each_with_index do |user, i|
      insert(db, 'api_keys', { id: i + 1, user_id: user, exchange_id: venues[i % venues.size], status: 1, key_type: i % 3,
                               key: "#{PLANTED}key-#{i}", secret: "#{PLANTED}secret-#{i}", passphrase: "#{PLANTED}pass-#{i}",
                               access_token: "#{PLANTED}access-#{i}", rsa_signature_key: "#{PLANTED}rsa-sig-#{i}",
                               rsa_encryption_key: "#{PLANTED}rsa-enc-#{i}", dh_param: "#{PLANTED}dh-#{i}" }.merge(t))
    end
    venues.first(2).each { |v| insert(db, 'fee_api_keys', { exchange_id: v, key: "#{PLANTED}fee-key-#{v}", secret: "#{PLANTED}fee-secret-#{v}", passphrase: "#{PLANTED}fee-pass-#{v}" }) }
    configs = Export::Portable::APP_CONFIG_KEYS.map { |key| [key, "#{PLANTED}#{key}"] } +
              [['proxy_binance', "#{PLANTED}proxy"], ['smtp_future_option', "#{PLANTED}smtp-future"],
               ['registration_open', 'true'], ['setup_sync_status', 'done']]
    configs.each { |key, value| insert(db, 'app_configs', { key: key, value: value }.merge(t6)) }

    # Bots: every status, several kinds, the labels above; each with closed orders (two at one instant), an open one,
    # a failed one, and one of another venue (named by its venue in the file).
    statuses = [0, 1, 2, 3, 4, 5, 6, 7]
    types = %w[Bots::DcaSingleAsset Bots::DcaMultiAsset Bots::DcaIndex Bots::Signal]
    LABELS.each_with_index do |label, i|
      id = i + 1
      user = users[i % users.size]
      venue = venues[i % venues.size]
      insert(db, 'bots', { id: id, user_id: user, exchange_id: venue, type: types[i % types.size], status: statuses[i % statuses.size],
                           label: label, settings: { 'quote_amount' => 10 + i, 'interval' => 'day' }.to_json,
                           transient_data: '{}' }.merge(t))
      [['closed', 2, '2026-02-01 10:00:00', 'market_order', 'buy'], ['closed', 2, '2026-02-01 10:00:00', 'limit_order', 'sell'],
       ['closed', 2, "2026-0#{(i % 9) + 1}-15 23:59:59", 'market_order', 'buy'], ['open', 1, '2026-03-01 00:00:00', 'limit_order', 'buy']].each_with_index do |(name, ext, at, type, side), n|
        insert(db, 'transactions', { bot_id: id, exchange_id: n == 2 && i.odd? ? venues[(i + 1) % venues.size] : venue,
                                     external_id: n == 1 && i == 2 ? '=HYPERLINK("x")' : "#{name}-#{id}-#{n}", status: 0, external_status: ext,
                                     order_type: type == 'market_order' ? 0 : 1, side: side == 'buy' ? 0 : 1,
                                     amount: ['0.001', '1', '123456789.123456789', '0.000000001'][n], amount_exec: n == 1 ? nil : ['0.001', nil, '2.5'][n],
                                     price: ['64000', '0.5', '1e-10', '100'][n], quote_amount: n.zero? ? '64' : nil,
                                     quote_amount_exec: n == 2 ? '250.25' : nil, base: %w[btc eth sol aapl][n], quote: %w[usd eur usdt usd][n],
                                     created_at: at, updated_at: at })
      end
      insert(db, 'transactions', { bot_id: id, exchange_id: venue, external_id: "failed-#{id}", status: 1, external_status: 2,
                                   order_type: 0, side: 0, amount: '1', price: '1', base: 'btc', quote: 'usd', created_at: NOW, updated_at: NOW })
    end
    insert(db, 'bot_signals', { bot_id: 4, amount: 10, token: "#{PLANTED}signal-a", enabled: 1 }.merge(t6))
    insert(db, 'bot_signals', { bot_id: 9, amount: 5, token: "#{PLANTED}signal-b", enabled: 0, direction: 1 }.merge(t6))
    # Rules of every status: the withdrawal addresses are kept.
    statuses.each_with_index do |status, i|
      insert(db, 'rules', { id: i + 1, user_id: users[i % users.size], exchange_id: venues[i % venues.size],
                            type: 'Rules::Withdrawal', status: status,
                            address: i == 7 ? nil : "kept-withdrawal-address-#{i}", settings: '{}' }.merge(t6))
    end

    # The tracker: every entry type, several users and venues, CsvSafe-relevant text, and ties on transacted_at.
    texts = ['=SUM(A1:A2)', '+1', '-2', '@cmd', "\ttab", 'with, comma', 'say "hi"', "two\nlines", '', "'apostrophe", 'plain', nil]
    amounts = ['1', '0.1', '123456789.123456789', '0.000000001', '1e-10', '5000', '-3.25', '0', '99999999999.99']
    times = ['2026-01-05 00:00:00', '2026-01-05 00:00:00', '2026-01-05 12:30:15.123456', '2026-01-04 23:59:59.999999',
             '2026-01-05 00:00:00', '2026-02-01 08:00:00']
    AccountTransaction.entry_types.each_with_index do |(name, value), i|
      2.times do |k|
        quoted = (i + k).even?
        insert(db, 'account_transactions', { user_id: users[(i + k) % users.size], exchange_id: venues[(i + k) % venues.size],
                                             api_key_id: k.zero? ? (i % users.size) + 1 : nil, entry_type: value,
                                             base_currency: i == 3 ? '-BTC' : %w[BTC ETH AAPL USD][i % 4], base_amount: amounts[(i + k) % amounts.size],
                                             quote_currency: quoted ? 'USD' : nil, quote_amount: quoted ? amounts[(i + 2) % amounts.size] : nil,
                                             fee_currency: i % 3 == 0 ? 'USD' : nil, fee_amount: i % 3 == 0 ? amounts[(i + 5) % amounts.size] : nil,
                                             tx_id: i == 5 ? nil : "#{name}-#{i}-#{k}", group_id: i % 4 == 0 ? "group-#{i / 4}" : nil,
                                             description: texts[(i + k) % texts.size], transacted_at: times[(i + k) % times.size],
                                             raw_data: '{}', manual_values: '{}' }.merge(t6))
      end
    end

    # Sessions, OAuth and MCP: every row goes.
    insert(db, 'oauth_applications', { id: 1, name: 'client', uid: 'client-id', secret: "#{PLANTED}oauth-secret",
                                       registration_access_token: "#{PLANTED}registration", redirect_uri: 'https://example.test/cb' }.merge(t6))
    insert(db, 'oauth_access_tokens', { application_id: 1, resource_owner_id: 1, token: "#{PLANTED}bearer", refresh_token: "#{PLANTED}refresh", created_at: NOW6 })
    insert(db, 'oauth_access_grants', { application_id: 1, resource_owner_id: 1, token: "#{PLANTED}grant", expires_in: 600,
                                        redirect_uri: 'https://example.test/cb', created_at: NOW6 })
    insert(db, 'connected_clients', { user_id: 1, oauth_application_id: 1, mcp_tools: "[\"#{PLANTED}tools\"]" }.merge(t6))
    insert(db, 'action_mcp_sessions', { id: "#{PLANTED}session", session_data: "{\"token\":\"#{PLANTED}session-data\"}" }.merge(t6))
    insert(db, 'action_mcp_session_messages', { session_id: "#{PLANTED}session", message_type: 'request', message_json: "{\"token\":\"#{PLANTED}message\"}" }.merge(t6))
    insert(db, 'action_mcp_session_subscriptions', { session_id: "#{PLANTED}session", uri: "#{PLANTED}uri" }.merge(t6))
    insert(db, 'action_mcp_session_tasks', { id: "#{PLANTED}task", session_id: "#{PLANTED}session", last_updated_at: NOW6,
                                             request_params: "{\"token\":\"#{PLANTED}task-params\"}" }.merge(t6))
    db.execute('PRAGMA wal_checkpoint(TRUNCATE)')
    ActiveRecord::Base.connection_pool.disconnect!
  end

  # A value as JSON carries it: a REAL as Float#to_s writes it, a BLOB as hex.
  def cell(v)
    case v
    when Float then { 'real' => v.to_s }
    when SQLite3::Blob then { 'blob' => v.unpack1('H*') }
    when String then v.encoding == Encoding::BINARY && !v.dup.force_encoding('UTF-8').valid_encoding? ? { 'blob' => v.unpack1('H*') } : v.dup.force_encoding('UTF-8')
    else v
    end
  end

  # Every table but SQLite's own: its columns in order and its rows by rowid.
  def tables(path)
    db = SQLite3::Database.new(path, readonly: true)
    names = db.execute("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name").flatten
    names.to_h do |name|
      columns, *rows = db.execute2("SELECT * FROM \"#{name}\" ORDER BY rowid")
      [name, { 'columns' => columns, 'rows' => rows.map { |row| row.map { |v| cell(v) } } }]
    end
  ensure
    db&.close
  end

  def run_process(mode, dir)
    env = { 'SECRET_KEY_BASE' => SECRET_KEY_BASE, 'EXPORT_SCRATCH' => '1', 'RAILS_ENV' => 'production' }
    command = <<~SH
      set -e
      DELTABADGER_ENTRYPOINT_SOURCED=1
      . "$1"
      SECRETS_FILE="$2"
      if [ -f "$SECRETS_FILE" ]; then load_secrets >&2; fi
      exec "$3" "$4" "$5" "$6"
    SH
    stdout, stderr, status = Open3.capture3(
      env, '/bin/bash', '-c', command, 'export-recorder', Rails.root.join('docker-entrypoint.sh').to_s,
      File.join(dir, 'storage/.secrets'), RbConfig.ruby, Rails.root.join('test/support/portable_export_process.rb').to_s, mode, dir,
      unsetenv_others: true, chdir: Rails.root.to_s
    )
    raise "#{mode} failed (#{status.exitstatus}):\n#{stdout}\n#{stderr}" unless status.success?

    stdout
  end

  def scratch(dir)
    FileUtils.mkdir_p(File.join(dir, 'storage'))
    File.write(File.join(dir, '.dockerenv'), '')
  end

  def placeholders(text)
    text.gsub(Rails.application.config.version, '{{app_version}}')
        .gsub(ActiveRecord::Base.connection_pool.migration_context.current_version.to_s, '{{schema_version}}')
  end

  def run(out)
    Dir.mktmpdir('portable-export') do |dir|
      scratch(dir)
      primary = File.join(dir, 'storage/production.sqlite3')
      seed(primary)
      source = tables(primary)
      printed = run_process('export', dir)
      raise 'no completion line' unless printed.include?('Portable export complete: /export/manifest.json')

      export = File.join(dir, 'export')
      files = Dir.glob('**/*', base: export).select { |f| File.file?(File.join(export, f)) }.sort
      bots = files.grep(%r{\Acsv/bots/}).to_h { |f| [f.delete_prefix('csv/bots/'), File.read(File.join(export, f))] }
      recorded = {
        'files' => files,
        'printed' => placeholders(printed),
        'manifest' => placeholders(File.read(File.join(export, 'manifest.json'))),
        'readme_csv' => placeholders(File.read(File.join(export, 'csv/README_CSV.txt'))),
        'tracker_csv' => File.read(File.join(export, 'csv/tracker.csv')),
        'bots' => bots,
        'source' => source,
        'scrubbed' => tables(File.join(export, 'portable/production.sqlite3')),
        'app_config_keys' => Export::Portable::APP_CONFIG_KEYS,
        'app_config_prefixes' => Export::Portable::APP_CONFIG_PREFIXES,
        'session_tables' => Export::Portable::SESSION_TABLES,
        'api_key_columns' => EncryptedCredentials::COVERAGE.fetch('ApiKey').fetch(:attributes).map(&:to_s),
        'transliteration' => I18n::Backend::Transliterator::HashTransliterator::DEFAULT_APPROXIMATIONS,
        'labels' => LABELS.to_h { |l| [l.to_s, l.to_s.parameterize.gsub(/[^a-z0-9-]/, '').first(50).presence || 'bot'] }
      }
      # Restore 5.2: Rails' own seeded install (its September schema), exported by Rails, primary kept gzipped.
      Dir.mktmpdir('portable-export-2x') do |dir2x|
        scratch(dir2x)
        File.write(File.join(dir2x, 'storage/production.sqlite3'), '')
        run_process('seed', dir2x)
        run_process('export', dir2x)
        gz = out.sub(/\.json\z/, '_2x.sqlite3.gz')
        Zlib::GzipWriter.open(gz) do |w|
          w.mtime = 1
          w.write(File.binread(File.join(dir2x, 'export/portable/production.sqlite3')))
        end
      end
      recorded
    end
  end
end

out = ARGV.fetch(0)
recorded = PortableExport.run(out)
File.write(out, "#{JSON.pretty_generate(recorded.merge(
  'sources' => PortableExport::PORTED.to_h { |path| [path, Digest::SHA256.file(Rails.root.join(path)).hexdigest] }
))}\n")
puts "#{recorded['bots'].size} bot files -> #{out}"
