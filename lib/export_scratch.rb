# Checked before Rails boots and again before any database is changed. EXPORT_ENTRYPOINT
# is set by the export branch, not required from the caller's environment.
module ExportScratch
  STORAGE = '/app/storage'.freeze
  OUTPUT = '/export'.freeze
  CONTAINER_MARKER = '/.dockerenv'.freeze
  DATABASE_FILES = %w[production.sqlite3 production_queue.sqlite3 production_cache.sqlite3 production_cable.sqlite3].freeze

  class UnsafeEnvironment < StandardError; end

  def self.active?
    ENV['EXPORT_ENTRYPOINT'] == '1' && ENV['EXPORT_SCRATCH'] == '1'
  end

  def self.validate!
    unless active? && ENV['RAILS_ENV'] == 'production' && File.file?(CONTAINER_MARKER)
      raise UnsafeEnvironment, 'Export requires the export entrypoint in a production container with EXPORT_SCRATCH=1'
    end

    overrides = %w[DATABASE_URL DATABASE_PATH PRIMARY_DATABASE_URL QUEUE_DATABASE_URL CACHE_DATABASE_URL CABLE_DATABASE_URL
                   QUEUE_DATABASE_PATH CACHE_DATABASE_PATH CABLE_DATABASE_PATH]
    raise UnsafeEnvironment, 'Export does not accept database path overrides' if overrides.any? { |key| !ENV[key].to_s.empty? }
    unless File.directory?(STORAGE) && File.realpath(STORAGE) == STORAGE
      raise UnsafeEnvironment, 'Export storage must be /app/storage without symlinks'
    end

    paths = DATABASE_FILES.flat_map { |file| [file, "#{file}-wal", "#{file}-shm", "#{file}-journal"] } + %w[.secrets .engine.lock]
    paths.each do |file|
      path = File.join(STORAGE, file)
      if File.symlink?(path) || (File.exist?(path) && (!File.file?(path) || File.stat(path).nlink > 1))
        raise UnsafeEnvironment, 'Export storage contains a linked or non-regular database or secrets file'
      end
    end
    raise UnsafeEnvironment, 'Export requires a copied primary database' unless File.file?(File.join(STORAGE, DATABASE_FILES.first))
  end
end
