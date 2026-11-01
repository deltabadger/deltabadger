# The two empty databases every scenario starts from, built ONCE per recorder process and then copied, instead of
# built again for each scenario: the primary by `load_primary`, the queue from db/queue_schema.rb.
# The template's connections are checkpointed and closed before the first copy, so no committed page is
# left in a -wal file; each scenario then opens its own fresh connections. rust/tests/rails_templates.rs
# runs script/rust/schema_template_check.rb, which compares a copy with a per-scenario load.
require 'fileutils'
require 'tmpdir'

module SchemaTemplate
  PRIMARY, QUEUE = 'production.sqlite3', 'production_queue.sqlite3'
  FILES = [PRIMARY, QUEUE].freeze

  # Every recorder's primary schema, connected to `path` afterwards. B1_SCHEMA_SOURCE=baseline (the default) executes
  # rust/schema/baseline.sql as store::create_schema does (WAL, then one BEGIN IMMEDIATE); =rails loads db/schema.rb,
  # for the B1-3 proof that the grids answer the same on both.
  def self.load_primary(path)
    ActiveRecord::Base.establish_connection(adapter: 'sqlite3', database: path)
    case (source = ENV.fetch('B1_SCHEMA_SOURCE', 'baseline'))
    when 'baseline'
      db = ActiveRecord::Base.connection.raw_connection
      db.execute('PRAGMA journal_mode=WAL')
      db.transaction(:immediate) { db.execute_batch(File.read(Rails.root.join('rust/schema/baseline.sql'))) }
    when 'rails'
      ActiveRecord::Schema.verbose = false
      load Rails.root.join('db/schema.rb')
    else raise "B1_SCHEMA_SOURCE=#{source}: baseline or rails"
    end
  end

  def self.load_queue(path)
    ActiveRecord::Base.establish_connection(adapter: 'sqlite3', database: path)
    ActiveRecord::Schema.verbose = false
    load Rails.root.join('db/queue_schema.rb')
  end

  # Writes both databases into dir, as building them there would.
  def self.install(dir)
    FileUtils.mkdir_p(dir)
    FILES.each { |file| FileUtils.cp(File.join(template, file), File.join(dir, file)) }
  end

  # The owner's encrypted_password: Devise's bcrypt of the recorders' one password (cost 11 outside the test
  # environment, 0.3 s a hash), hashed once per process instead of once per scenario. Another hash of the same
  # password differs only in its random salt, as every fresh recording's already does.
  def self.owner_password(password)
    (@owner_passwords ||= {})[password] ||= User.new(password:).encrypted_password
  end

  def self.template
    @template ||= begin
      dir = Dir.mktmpdir('schema-template')
      at_exit { FileUtils.rm_rf(dir) }
      FILES.each do |file|
        path = File.join(dir, file)
        file == PRIMARY ? load_primary(path) : load_queue(path)
        ActiveRecord::Base.connection.execute('PRAGMA wal_checkpoint(TRUNCATE)')
        ActiveRecord::Base.connection_pool.disconnect!
        wal = "#{path}-wal"
        raise "#{wal} still holds pages after the template closed" if File.exist?(wal) && File.size(wal).positive?
      end
      dir
    end
  end
end

# `bin/rails runner script/rust/schema_template.rb`: every database of this environment with its schema, the primary
# by `load_primary`, the others from their schema files (db/queue_schema.rb, ...), as the recorders' boot.
if File.expand_path($PROGRAM_NAME) == __FILE__
  ActiveRecord::Base.configurations.configs_for(env_name: Rails.env).each do |config|
    next SchemaTemplate.load_primary(config.database) if config.name == 'primary'

    ActiveRecord::Base.establish_connection(adapter: 'sqlite3', database: config.database)
    ActiveRecord::Schema.verbose = false
    load Rails.root.join(ActiveRecord::Tasks::DatabaseTasks.schema_dump_path(config))
  end
  ActiveRecord::Base.connection_pool.disconnect!
end
