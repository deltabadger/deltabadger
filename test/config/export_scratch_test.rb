require 'test_helper'
require 'tmpdir'

class ExportScratchTest < ActiveSupport::TestCase
  setup do
    require Rails.root.join('lib/export_scratch')
  end

  test 'requires the export entrypoint, production, scratch flag and container marker' do
    with_scratch do
      %w[EXPORT_ENTRYPOINT EXPORT_SCRATCH RAILS_ENV].each do |key|
        original = ENV.delete(key)
        assert_raises(ExportScratch::UnsafeEnvironment, key) { ExportScratch.validate! }
        ENV[key] = original
      end
      File.delete(ExportScratch::CONTAINER_MARKER)
      assert_raises(ExportScratch::UnsafeEnvironment) { ExportScratch.validate! }
    end
  end

  test 'rejects database overrides and symlinks before opening databases' do
    with_scratch do
      ENV['DATABASE_URL'] = 'sqlite3:/outside/production.sqlite3'
      assert_raises(ExportScratch::UnsafeEnvironment) { ExportScratch.validate! }
      ENV.delete('DATABASE_URL')
      path = File.join(ExportScratch::STORAGE, 'production.sqlite3')
      File.delete(path)
      File.symlink('/outside/production.sqlite3', path)
      assert_raises(ExportScratch::UnsafeEnvironment) { ExportScratch.validate! }
    end
  end

  private

  def with_scratch(&block)
    original = ENV.to_h
    Dir.mktmpdir do |dir|
      storage = File.join(dir, 'storage')
      FileUtils.mkdir_p(storage)
      marker = File.join(dir, '.dockerenv')
      File.write(marker, '')
      File.write(File.join(storage, 'production.sqlite3'), '')
      %w[DATABASE_URL PRIMARY_DATABASE_URL QUEUE_DATABASE_URL CACHE_DATABASE_URL CABLE_DATABASE_URL].each { |key| ENV.delete(key) }
      ENV.update('EXPORT_ENTRYPOINT' => '1', 'EXPORT_SCRATCH' => '1', 'RAILS_ENV' => 'production')
      stub_const(ExportScratch, :STORAGE, File.realpath(storage)) do
        stub_const(ExportScratch, :CONTAINER_MARKER, marker, &block)
      end
    end
  ensure
    ENV.replace(original)
  end
end
