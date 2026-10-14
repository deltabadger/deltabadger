# The two empty databases every scenario starts from, loaded from db/schema.rb and db/queue_schema.rb ONCE
# per recorder process and then copied, instead of loaded again for each scenario.
# The template's connections are checkpointed and closed before the first copy, so no committed page is
# left in a -wal file; each scenario then opens its own fresh connections. rust/tests/rails_templates.rs
# runs script/rust/schema_template_check.rb, which compares a copy with a per-scenario load.
require 'fileutils'
require 'tmpdir'

module SchemaTemplate
  FILES = { 'production.sqlite3' => 'db/schema.rb', 'production_queue.sqlite3' => 'db/queue_schema.rb' }.freeze

  # Writes both databases into dir, as loading the two schema files there would.
  def self.install(dir)
    FileUtils.mkdir_p(dir)
    FILES.each_key { |file| FileUtils.cp(File.join(template, file), File.join(dir, file)) }
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
      ActiveRecord::Schema.verbose = false
      FILES.each do |file, schema|
        path = File.join(dir, file)
        ActiveRecord::Base.establish_connection(adapter: 'sqlite3', database: path)
        load Rails.root.join(schema)
        ActiveRecord::Base.connection.execute('PRAGMA wal_checkpoint(TRUNCATE)')
        ActiveRecord::Base.connection_pool.disconnect!
        wal = "#{path}-wal"
        raise "#{wal} still holds pages after the template closed" if File.exist?(wal) && File.size(wal).positive?
      end
      dir
    end
  end
end
