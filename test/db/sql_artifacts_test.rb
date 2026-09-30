require 'test_helper'
require 'open3'
require 'sqlite3'
require 'zlib'

# db/sql/ is how the Rust backend (rust/) creates and upgrades this app's databases without Ruby.
# Rails stays the schema author; these tests fail whenever the SQL no longer says what Rails says.
# Each generator run boots Rails in a subprocess, so the app's own test database is never touched.
class SqlArtifactsTest < ActiveSupport::TestCase
  SQL = Rails.root.join('db/sql')

  # The runner boots Rails before the script can switch connections, and boot touches databases
  # (e.g. Solid Queue's stale-record cleanup), so every database it could open points into a scratch dir.
  def generate(*args)
    Dir.mktmpdir do |boot|
      env = { 'SMTP_ADDRESS' => nil, 'MARKET_DATA_URL' => nil, 'MARKET_DATA_TOKEN' => nil, 'DATABASE_URL' => nil,
              **%w[primary queue cache cable].to_h { |db| ["#{db.upcase}_DATABASE_URL", "sqlite3:#{boot}/#{db}.sqlite3"] } }
      out, status = Open3.capture2e(env, 'bin/rails', 'runner', 'script/sql_artifacts.rb', *args, chdir: Rails.root.to_s)
      assert status.success?, out
    end
  end

  def migration_versions = Dir[Rails.root.join('db/migrate/*.rb')].map { |f| File.basename(f)[/\A\d+/] }
  def twins = Dir[SQL.join('migrate/*.sql')]
  def baseline_versions = SQL.join('primary_baseline.sql').read.scan(/"schema_migrations" \("version"\) VALUES \('(\d+)'\)/).flatten

  test 'every migration is in the primary baseline or has an SQL twin, and every twin has a migration' do
    twin_versions = twins.map { |f| File.basename(f)[/\A\d+/] }
    assert_empty migration_versions - baseline_versions - twin_versions, 'add a db/sql/migrate twin for these migrations'
    assert_empty twin_versions - migration_versions, 'these twins have no migration'
  end

  test 'the primary baseline plus twins reproduces db/schema.rb' do
    Dir.mktmpdir do |dir|
      db_path = File.join(dir, 'primary.sqlite3')
      db = SQLite3::Database.new(db_path)
      db.transaction do
        db.execute_batch(SQL.join('primary_baseline.sql').read)
        twins.each do |twin|
          db.execute_batch(File.read(twin))
          db.execute('INSERT INTO schema_migrations (version) VALUES (?)', [File.basename(twin)[/\A\d+/]])
        end
      end
      db.close
      generate('dump_schema', db_path, File.join(dir, 'schema.rb'))
      strip = ->(s) { s.lines.reject { |l| l.start_with?('#') }.join }
      assert_equal strip.call(Rails.root.join('db/schema.rb').read), strip.call(File.read(File.join(dir, 'schema.rb')))
    end
  end

  test 'the queue, cache and cable baselines and the seed are current' do
    Dir.mktmpdir do |dir|
      generate('generate', dir)
      %w[queue cache cable].each do |name|
        assert_equal SQL.join("#{name}_baseline.sql").read, File.read(File.join(dir, "#{name}_baseline.sql")),
                     "db/sql/#{name}_baseline.sql is stale: run bin/rails db:sql:baseline"
      end
      assert_equal Zlib.gunzip(SQL.join('seed.sql.gz').binread), Zlib.gunzip(File.binread(File.join(dir, 'seed.sql.gz'))),
                   'db/sql/seed.sql.gz is stale: run bin/rails db:sql:seed'
    end
  end

  # Every twin must prove it does what its Ruby migration does to DATA, not only to the schema: a
  # comment-only twin for a data migration would pass the schema comparison and mark the change applied.
  test 'every twin has a data-equivalence test' do
    twins.each do |twin|
      version = File.basename(twin)[/\A\d+/]
      assert Rails.root.join("test/db/sql_twins/#{version}_test.rb").exist?,
             "add test/db/sql_twins/#{version}_test.rb: run the migration and #{File.basename(twin)} on the same populated " \
             'fixture with foreign keys on and compare every table the migration touches'
    end
  end

  test 'the seed applies to the baseline plus twins with foreign keys enforced' do
    Dir.mktmpdir do |dir|
      db = SQLite3::Database.new(File.join(dir, 'seeded.sqlite3'))
      db.execute('PRAGMA foreign_keys = ON')
      db.transaction do
        db.execute_batch(SQL.join('primary_baseline.sql').read)
        twins.each { |twin| db.execute_batch(File.read(twin)) } # the seed is generated from the CURRENT schema
        db.execute_batch(Zlib.gunzip(SQL.join('seed.sql.gz').binread).force_encoding(Encoding::UTF_8))
      end
      assert_empty db.execute('PRAGMA foreign_key_check')
      assert_operator db.get_first_value("SELECT count(*) FROM exchanges WHERE type = 'Exchanges::Kraken'"), :==, 1
    end
  end
end
