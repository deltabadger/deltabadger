# Proves script/rust/schema_template.rb equivalent to loading db/schema.rb and db/queue_schema.rb per scenario, for
# rust/tests/rails_templates.rs:
#   bin/rails runner script/rust/schema_template_check.rb <dir>
# Two installs are compared: one loaded per scenario as the recorders did, and one copied from the template. Their
# schema, every row (ar_internal_metadata's two timestamps say when each was loaded, and nothing reads them), the
# file-level pragmas, sqlite_sequence and the next id each table hands out must be the same. A real insert into each
# database, through fresh connections, must then get the same id in both.
require_relative 'schema_template'

root = ARGV.fetch(0)
loaded = File.join(root, 'loaded')
copied = File.join(root, 'copied')
FileUtils.mkdir_p(loaded)
ActiveRecord::Schema.verbose = false
SchemaTemplate::FILES.each do |file, schema|
  ActiveRecord::Base.establish_connection(adapter: 'sqlite3', database: File.join(loaded, file))
  load Rails.root.join(schema)
  ActiveRecord::Base.connection_pool.disconnect!
end
SchemaTemplate.install(copied)

def snapshot(path)
  ActiveRecord::Base.establish_connection(adapter: 'sqlite3', database: path)
  c = ActiveRecord::Base.connection
  tables = c.select_values("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
  out = {
    'schema' => c.select_rows('SELECT type, name, tbl_name, sql FROM sqlite_master ORDER BY type, name'),
    'pragmas' => %w[journal_mode page_size auto_vacuum encoding user_version application_id].to_h { |p| [p, c.select_value("PRAGMA #{p}")] },
    'rows' => tables.to_h do |t|
      rows = c.select_all("SELECT * FROM #{c.quote_table_name(t)}").to_a
      rows = rows.map { |r| r.except('created_at', 'updated_at') } if t == 'ar_internal_metadata'
      [t, rows.sort_by(&:to_s)]
    end,
    # The next id a plain INSERT gets: AUTOINCREMENT tables continue from sqlite_sequence, the others from max(rowid).
    'next_rowid' => tables.reject { |t| t.start_with?('sqlite_') }.to_h do |t|
      [t, c.select_value("SELECT coalesce(max(rowid), 0) FROM #{c.quote_table_name(t)}")]
    end
  }
  ActiveRecord::Base.connection_pool.disconnect!
  out
end

def inserted_ids(dir)
  ActiveRecord::Base.establish_connection(adapter: 'sqlite3', database: File.join(dir, 'production.sqlite3'))
  asset = Asset.create!(external_id: 'bitcoin', symbol: 'BTC', name: 'Bitcoin', category: 'Cryptocurrency').id
  ActiveRecord::Base.connection_pool.disconnect!
  ActiveRecord::Base.establish_connection(adapter: 'sqlite3', database: File.join(dir, 'production_queue.sqlite3'))
  c = ActiveRecord::Base.connection
  c.execute("INSERT INTO solid_queue_processes (kind, last_heartbeat_at, pid, hostname, created_at, name) VALUES ('Worker', '2026-09-01 00:00:00', 1, 'h', '2026-09-01 00:00:00', 'w')")
  process = c.select_value('SELECT last_insert_rowid()')
  ActiveRecord::Base.connection_pool.disconnect!
  [asset, process]
end

SchemaTemplate::FILES.each_key do |file|
  %w[-wal -shm -journal].each { |s| raise "#{file}#{s} copied with the template" if File.exist?(File.join(copied, file + s)) }
  a = snapshot(File.join(loaded, file))
  b = snapshot(File.join(copied, file))
  a.each_key { |k| raise "#{file}: #{k} differs\n loaded: #{a[k].inspect[0, 2000]}\n copied: #{b[k].inspect[0, 2000]}" unless a[k] == b[k] }
  raise "#{file}: no tables loaded" if a['rows'].size < 2
end
ids = [loaded, copied].map { |d| inserted_ids(d) }
raise "next ids differ: loaded #{ids[0]}, copied #{ids[1]}" unless ids[0] == ids[1]
puts "equivalent: #{SchemaTemplate::FILES.keys.join(', ')}, next ids #{ids[0].inspect}"
