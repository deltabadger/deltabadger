# Behind guard (PLAN-B3 §4, A+), run by the 2.70.21 image before 3.0 starts: 2.70.21's migrate, only on an install
# missing one of 2.70.21's migrations. Never boots Rails: a Rails boot refuses a 3.0-owned install, adopts a handed-back
# one and creates an install on an empty volume. Umbrel's compose carries these lines verbatim: no dollar sign anywhere.
require "sqlite3"
primary = ENV.fetch("DATABASE_PATH", "storage/production.sqlite3")
exit 0 unless File.file?(primary)
recorded = begin
  db = SQLite3::Database.new(primary, readonly: true)
  db.execute("SELECT version FROM schema_migrations").flatten.tap { db.close }
rescue SQLite3::Exception => e
  warn "Deltabadger: #{primary} can't be read (#{e.message}); leaving it for Deltabadger to explain"
  exit 0
end
missing = Dir.children("db/migrate").filter_map { |file| file[/\A\d+/] } - recorded
exit 0 if missing.empty?
puts "Deltabadger: your data is from an older version, so it's being updated first. This can take a few minutes."
STDOUT.flush
exit 0 if system("docker-entrypoint.sh", "migrate")
warn "Deltabadger couldn't finish updating your data. Nothing was bought or sold. Restart the app to try again; if it keeps failing, contact support."
exit 1
