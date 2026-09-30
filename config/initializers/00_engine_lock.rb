# Named 00_ to load first: the lock must be held before anything can read or write a database.
# Another engine (the Rust backend in rust/) can own this install's data, and two engines on one
# install would trade every bot twice. Every process of this app holds a SHARED lock on .engine.lock
# (next to the primary database) for its whole life; the other engine takes it EXCLUSIVE, so the two
# can never run together. Forked Puma workers inherit it. See engine_lease.rb for the handover record.
module EngineLease
  KEY = 'engine_lease'.freeze

  class HeldError < StandardError; end

  module_function

  # From ActiveRecord's RESOLVED configuration, so DATABASE_URL / PRIMARY_DATABASE_URL overrides are
  # honoured: the lock must sit next to the file this process actually opens.
  def primary_dir
    database = ActiveRecord::Base.configurations.configs_for(env_name: Rails.env, name: 'primary').database
    File.dirname(File.expand_path(database, Rails.root))
  end

  def lock!(dir)
    FileUtils.mkdir_p(dir)
    file = File.open(File.join(dir, '.engine.lock'), File::RDONLY | File::CREAT, 0o666) # rubocop:disable Style/FileOpen -- held for the process lifetime
    unless file.flock(File::LOCK_SH | File::LOCK_NB)
      file.close
      raise HeldError, 'The Deltabadger Rust engine is running on this data. Stop it and run `deltabadger handback` first.'
    end
    @lock = file # held until the process exits
  end
end

# Not in tests (they call the pieces directly), not in a console (read-only inspection must stay
# possible while the other engine runs), not in the asset-precompile build step (no data).
EngineLease.lock!(EngineLease.primary_dir) unless Rails.env.test? || defined?(Rails::Console) || ENV['SECRET_KEY_BASE_DUMMY'].present?
