# Records what store::seed must reproduce (PLAN-B1 T5): `Rails.application.load_seed` (db/seeds.rb) on a fresh primary
# built from rust/schema/baseline.sql (SchemaTemplate.load_primary), time frozen, once per SMTP_ADDRESS/MARKET_DATA_URL
# combination, then every table of the result, every row in rowid order.
#   bin/rails runner script/rust/record_seeds.rb rust/tests/fixtures/seeds_rails.json
# A cell is "<typeof>:<text>" as SQLite stores it (a real as its IEEE-754 bits in hex), NULL as null; rust/tests/seeds.rs
# reads its own database with the same SELECT. app_configs.value is recorded decrypted (its ciphertext is drawn fresh);
# ar_internal_metadata's timestamps are the baseline's, not the seed's, and are left out. The first scenario holds
# every table; a later one only the tables whose rows differ from it. Re-run when a file under "sources" changes
# (script/rust/repin).
require 'json'
require 'digest'
require 'tmpdir'
require 'active_support/testing/time_helpers'
require_relative 'schema_template'

module RecordSeeds
  extend ActiveSupport::Testing::TimeHelpers
  module_function

  NOW = Time.utc(2026, 11, 1, 9, 30, 15, 123_456)
  ENV_KEYS = %w[SMTP_ADDRESS MARKET_DATA_URL].freeze
  SCENARIOS = {
    'neither' => {},
    'smtp_only' => { 'SMTP_ADDRESS' => 'smtp.example.com' },
    'market_data_only' => { 'MARKET_DATA_URL' => 'http://data.example' },
    'both' => { 'SMTP_ADDRESS' => 'smtp.example.com', 'MARKET_DATA_URL' => 'http://data.example' },
    # Ruby's present?: whitespace only is absent.
    'whitespace_only' => { 'SMTP_ADDRESS' => " \t", 'MARKET_DATA_URL' => "\n " }
  }.freeze
  PORTED = (%w[
    db/seeds.rb app/models/market_data.rb app/models/app_config.rb app/models/exchange.rb app/models/asset.rb
    app/models/market_data_settings.rb rust/schema/baseline.sql script/rust/record_seeds.rb script/rust/schema_template.rb
  ] + Dir.glob('db/seed_data/**/*.json', base: Rails.root).sort).freeze

  # A real by its IEEE-754 bits: SQLite's own printf of a double differs between versions.
  def cell(type, value)
    case type
    when 'null' then nil
    when 'real' then "real:#{[value].pack('G').unpack1('H*')}"
    when 'blob' then "blob:#{value.unpack1('H*')}"
    else "#{type}:#{value}"
    end
  end

  def dump(db)
    db.results_as_hash = false
    db.execute("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name").flatten.to_h do |table|
      columns = db.execute("SELECT name FROM pragma_table_info('#{table}') ORDER BY cid").flatten
      columns -= %w[created_at updated_at] if table == 'ar_internal_metadata'
      pairs = columns.map { %(typeof("#{_1}"), "#{_1}") }.join(', ')
      rows = db.execute("SELECT rowid, #{pairs} FROM \"#{table}\" ORDER BY rowid")
               .map { |r| ["integer:#{r[0]}", *r.drop(1).each_slice(2).map { |t, v| cell(t, v) }] }
      if table == 'app_configs'
        value = columns.index('value') + 1
        rows = rows.map { |r| r.dup.tap { _1[value] = (v = AppConfig.find(r[0].delete_prefix('integer:').to_i).value).nil? ? nil : "text:#{v}" } }
      end
      [table, rows]
    end
  end

  def scenario(env)
    Dir.mktmpdir('record-seeds') do |dir|
      path = File.join(dir, SchemaTemplate::PRIMARY)
      SchemaTemplate.load_primary(path)
      saved = ENV_KEYS.to_h { [_1, ENV.fetch(_1, nil)] }
      ENV_KEYS.each { ENV.delete(_1) }
      env.each { |k, v| ENV[k] = v }
      begin
        travel_to(NOW, with_usec: true) { Rails.application.load_seed }
      ensure
        saved.each { |k, v| v.nil? ? ENV.delete(k) : ENV[k] = v }
      end
      adapter = ActiveJob::Base.queue_adapter
      raise 'the seed enqueued a job' if adapter.respond_to?(:enqueued_jobs) && adapter.enqueued_jobs.any?

      out = dump(ActiveRecord::Base.connection.raw_connection)
      ActiveRecord::Base.connection_pool.disconnect!
      out
    end
  end

  def run(out_path)
    first = nil
    scenarios = SCENARIOS.map do |name, env|
      tables = scenario(env)
      first ||= tables
      { 'name' => name, 'env' => env, 'tables' => tables.reject { |t, rows| !first.equal?(tables) && first[t] == rows } }
    end
    sources = PORTED.to_h { [_1, Digest::SHA256.file(Rails.root.join(_1)).hexdigest] }
    File.write(out_path, render('now' => NOW.iso8601(6), 'scenarios' => scenarios, 'sources' => sources))
  end

  # Pretty JSON, but a table row on one line: a re-record's diff names the rows that moved.
  def render(top)
    j = ->(v) { JSON.generate(v) }
    scenarios = top['scenarios'].map do |s|
      tables = s['tables'].map { |t, rows| "      #{j.(t)}: [#{rows.map { "\n        #{j.(_1)}" }.join(',')}#{rows.empty? ? '' : "\n      "}]" }
      "    {\n      \"name\": #{j.(s['name'])},\n      \"env\": #{j.(s['env'])},\n      \"tables\": {\n#{tables.join(",\n").gsub(/^/, '  ')}\n      }\n    }"
    end
    sources = top['sources'].map { |k, v| "    #{j.(k)}: #{j.(v)}" }
    "{\n  \"now\": #{j.(top['now'])},\n  \"scenarios\": [\n#{scenarios.join(",\n")}\n  ],\n  \"sources\": {\n#{sources.join(",\n")}\n  }\n}\n"
  end
end

RecordSeeds.run(ARGV.fetch(0))
