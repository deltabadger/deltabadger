# Generates db/sql/, the SQL the Rust backend (rust/) uses to create and upgrade this app's databases.
#   bin/rails runner script/sql_artifacts.rb baseline_primary <out.sql>
#   bin/rails runner script/sql_artifacts.rb generate <dir>          # queue/cache/cable baselines + seed.sql.gz
#   bin/rails runner script/sql_artifacts.rb dump_schema <db> <out.rb>
# Every database it builds is a temporary file. Run it through lib/tasks/sql_artifacts.rake (or as the
# test does), which also points the *_DATABASE_PATH variables at scratch files, so the app's own databases
# are not opened even while Rails boots.
require 'sqlite3'
require 'tmpdir'
require 'zlib'

module SqlArtifacts
  SCHEMAS = { 'primary' => 'db/schema.rb', 'queue' => 'db/queue_schema.rb', 'cache' => 'db/cache_schema.rb',
              'cable' => 'db/cable_schema.rb' }.freeze
  BOOKKEEPING = %w[schema_migrations ar_internal_metadata].freeze
  NOT_SEEDED = (BOOKKEEPING + %w[app_configs]).freeze # app_configs defaults depend on the environment
  FIXED_TIME = '2026-01-01 00:00:00'.freeze

  module_function

  def literal(value)
    case value
    when nil then 'NULL'
    when Integer, Float then value.to_s
    when String then "'#{value.gsub("'", "''")}'"
    else raise ArgumentError, "unexpected #{value.class} in a seeded row"
    end
  end

  def tables(db) = db.execute("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY rowid").flatten

  def inserts(db, table)
    columns = db.execute("PRAGMA table_info(\"#{table}\")")
    names = columns.map { |c| %("#{c[1]}") }.join(', ')
    datetime = columns.map { |c| c[2].to_s.start_with?('datetime') }
    db.execute("SELECT * FROM \"#{table}\" ORDER BY rowid").map do |row|
      values = row.each_with_index.map { |v, i| datetime[i] && !v.nil? ? FIXED_TIME : v }
      %(INSERT INTO "#{table}" (#{names}) VALUES (#{values.map { |v| literal(v) }.join(', ')});)
    end
  end

  def baseline_sql(db)
    db.execute("UPDATE ar_internal_metadata SET value = 'production' WHERE key = 'environment'")
    ddl = db.execute("SELECT sql FROM sqlite_master WHERE sql IS NOT NULL AND name NOT LIKE 'sqlite_%' ORDER BY rowid")
            .map { |(sql)| "#{sql};" }
    "#{(ddl + BOOKKEEPING.flat_map { |t| inserts(db, t) }).join("\n")}\n"
  end

  def seed_sql(db)
    "#{(['PRAGMA defer_foreign_keys = ON;'] + (tables(db) - NOT_SEEDED).flat_map { |t| inserts(db, t) }).join("\n")}\n"
  end

  def fresh(schema, seed: false)
    Dir.mktmpdir do |dir|
      path = File.join(dir, 'fresh.sqlite3')
      ActiveRecord::Base.establish_connection(adapter: 'sqlite3', database: path)
      ActiveRecord::Schema.verbose = false
      load Rails.root.join(schema)
      load Rails.root.join('db/seeds.rb') if seed
      ActiveRecord::Base.connection_pool.disconnect!
      db = SQLite3::Database.new(path)
      begin
        yield db
      ensure
        db.close
      end
    end
  end

  def baseline_primary(out) = fresh(SCHEMAS['primary']) { |db| File.write(out, baseline_sql(db)) }

  def generate(dir)
    %w[queue cache cable].each do |name|
      fresh(SCHEMAS[name]) { |db| File.write(File.join(dir, "#{name}_baseline.sql"), baseline_sql(db)) }
    end
    fresh(SCHEMAS['primary'], seed: true) do |db|
      Zlib::GzipWriter.open(File.join(dir, 'seed.sql.gz')) do |gz|
        gz.mtime = 0
        gz.write(seed_sql(db))
      end
    end
  end

  def dump_schema(db_path, out)
    ActiveRecord::Base.establish_connection(adapter: 'sqlite3', database: db_path)
    io = StringIO.new
    ActiveRecord::SchemaDumper.dump(ActiveRecord::Base.connection_pool, io)
    File.write(out, io.string)
  end
end

command, *args = ARGV
raise ArgumentError, 'usage: baseline_primary <out> | generate <dir> | dump_schema <db> <out>' unless
  %w[baseline_primary generate dump_schema].include?(command)

SqlArtifacts.public_send(command, *args)
