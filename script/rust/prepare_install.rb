# Builds an install the way Rails prepares one, for the Rust tests (rust/tests/common/mod.rs):
#   bin/rails runner script/rust/prepare_install.rb <dir>
# Run it with every *_DATABASE_URL pointing at scratch files, as that helper does, so booting Rails
# opens none of the app's own databases.
dir = ARGV.fetch(0)
ActiveRecord::Schema.verbose = false
{ 'production.sqlite3' => 'db/schema.rb', 'production_queue.sqlite3' => 'db/queue_schema.rb' }.each do |file, schema|
  ActiveRecord::Base.establish_connection(adapter: 'sqlite3', database: File.join(dir, file))
  load Rails.root.join(schema)
  ActiveRecord::Base.connection_pool.disconnect!
end
puts "prepared #{dir}"
