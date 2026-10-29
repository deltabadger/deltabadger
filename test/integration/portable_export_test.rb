require 'test_helper'
require 'open3'
require 'tmpdir'

class PortableExportTest < ActiveSupport::TestCase
  SECRET = 'export-test-effective-secret-' * 4

  test 'offline production task exports scrubbed databases and CSV from a Rust-held database and retains migration jobs' do
    Dir.mktmpdir do |dir|
      FileUtils.mkdir_p(File.join(dir, 'storage'))
      File.write(File.join(dir, '.dockerenv'), '')
      File.write(File.join(dir, 'storage/production.sqlite3'), '')
      planted = JSON.parse(run_export_process('seed', dir).lines.last)
      output = run_export_process('export', dir)
      assert_includes output, 'Portable export complete: /export/manifest.json'

      export = File.join(dir, 'export')
      files = Dir.glob('**/*', base: export).select { |file| File.file?(File.join(export, file)) }.sort
      manifest = JSON.parse(File.read(File.join(export, 'manifest.json')))
      assert_equal Rails.application.config.version, manifest.fetch('app_version')
      assert_equal ActiveRecord::Base.connection_pool.migration_context.current_version, manifest.fetch('schema_version')
      assert_equal files - ['manifest.json'], manifest.fetch('files')
      assert_equal %w[csv/README_CSV.txt csv/bots/1-bad-label.csv csv/tracker.csv manifest.json
                      portable/production.sqlite3 portable/production_cable.sqlite3
                      portable/production_cache.sqlite3 portable/production_queue.sqlite3].sort, files
      files.each do |file|
        bytes = File.binread(File.join(export, file))
        planted.fetch('removed').each { |secret| refute_includes bytes, secret.b, "#{file} retained a credential" }
      end
      # Written CsvSafe-escaped, as every app export is; the Rust desktop importer unescapes it.
      tracker = Import::DeltabadgerCsv.new(File.read(File.join(export, 'csv/tracker.csv'))).entries
      assert_equal "'=formula description", tracker.fetch(0).fetch(:description)
      assert_equal(['closed-order'], CSV.read(File.join(export, 'csv/bots/1-bad-label.csv'), headers: true).map { |row| row['Order ID'] })
      readme = File.read(File.join(export, 'csv/README_CSV.txt'))
      %w[manual transfer raw submitted closed].each { |word| assert_includes readme, word }
      assert_includes readme, Rails.application.config.version

      restored = JSON.parse(run_export_process('restore', dir).lines.last)
      assert_equal 'KEPTOTPSECRET', restored.fetch('otp')
      assert_equal 'kept-withdrawal-address', restored.fetch('address')
      assert restored.fetch('password_valid')
      assert_equal [2], restored.fetch('bots')
      assert_equal [2], restored.fetch('rules')
      assert_equal [false], restored.fetch('signals')
      assert restored.fetch('api_key_empty')
      assert restored.fetch('users_cleared')
      assert restored.fetch('sessions_deleted')
      assert restored.fetch('fee_keys_deleted')
      assert restored.fetch('engine_lease_cleared')
      assert_equal ['AccountTransaction::ResolveAssetsJob'], restored.fetch('jobs')
      assert_equal 1, restored.fetch('ready_jobs')
      assert restored.fetch('cache_empty')
      assert restored.fetch('cable_empty')
      assert_equal 'true', restored.fetch('registration_open')
    end
  end

  test 'independent encryption keys preserve OTP and addresses with a corrected secrets file on restore' do
    Dir.mktmpdir do |dir|
      FileUtils.mkdir_p(File.join(dir, 'storage'))
      File.write(File.join(dir, '.dockerenv'), '')
      File.write(File.join(dir, 'storage/production.sqlite3'), '')
      keys = { 'ACTIVE_RECORD_ENCRYPTION_PRIMARY_KEY' => '1' * 64, 'ACTIVE_RECORD_ENCRYPTION_KEY_DERIVATION_SALT' => '2' * 64 }
      run_export_process('seed', dir, keys)
      run_export_process('export', dir, keys)
      secrets = "SECRET_KEY_BASE=#{SECRET}\n#{keys.map { |key, value| "#{key}=#{value}" }.join("\n")}\n"
      File.write(File.join(dir, 'export/portable/.secrets'), secrets)
      restored = JSON.parse(run_export_process('restore', dir, 'SECRET_KEY_BASE' => nil).lines.last)
      assert_equal 'KEPTOTPSECRET', restored.fetch('otp')
      assert_equal 'kept-withdrawal-address', restored.fetch('address')
      assert restored.fetch('password_valid')
    end
  end

  private

  def run_export_process(mode, dir, overrides = {})
    env = { 'SECRET_KEY_BASE' => SECRET, 'EXPORT_SCRATCH' => '1', 'RAILS_ENV' => 'production' }.merge(overrides)
    command = <<~SH
      set -e
      DELTABADGER_ENTRYPOINT_SOURCED=1
      . "$1"
      SECRETS_FILE="$2"
      if [ -f "$SECRETS_FILE" ]; then load_secrets >&2; fi
      exec "$3" "$4" "$5" "$6"
    SH
    secrets = File.join(dir, mode == 'restore' ? 'export/portable/.secrets' : 'storage/.secrets')
    stdout, stderr, status = Open3.capture3(
      env, '/bin/bash', '-c', command, 'export-test', Rails.root.join('docker-entrypoint.sh').to_s, secrets,
      RbConfig.ruby, Rails.root.join('test/support/portable_export_process.rb').to_s, mode, dir,
      unsetenv_others: true
    )
    assert status.success?, "#{mode} failed (#{status.exitstatus}):\n#{stdout}\n#{stderr}"
    stdout
  end
end
