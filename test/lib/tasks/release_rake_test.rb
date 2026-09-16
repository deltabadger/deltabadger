require 'test_helper'
require 'rake'

class ReleaseRakeTest < ActiveSupport::TestCase
  # See the comment in encryption_rake_test.rb for why this is `load` and not rake_require.
  setup do
    @argv = ARGV.dup
    Rake.application = Rake::Application.new
    Rake::Task.define_task(:environment)
    load Rails.root.join('lib/tasks/release.rake')
  end

  teardown do
    Rake.application = nil
    ARGV.replace(@argv)
  end

  # A major release is the one announcement nobody wants written from commit subjects, so the
  # task stops rather than falling back to a generated summary.
  test 'major refuses to release without a hand-written announcement' do
    ARGV.replace([])

    _out, err = capture_io do
      assert_raises(SystemExit) { Rake::Task['release:major'].invoke }
    end

    assert_match 'rails release:major notes.md', err
  end

  test 'major refuses a file that is not there' do
    ARGV.replace(['announcement.md'])

    _out, err = capture_io do
      assert_raises(SystemExit) { Rake::Task['release:major'].invoke }
    end

    assert_match 'announcement.md: no such file', err
  end

  # Rails checks every top-level task name before running any of them, so the file name has to
  # already be a task by the time release:major is looked up, or the release never starts.
  test 'the announcement file is not mistaken for a task to run' do
    ARGV.replace(['announcement.md'])

    load Rails.root.join('lib/tasks/release.rake')

    assert Rake::Task.task_defined?('announcement.md')
  end

  # Absolute, because the release runs `gh` from wherever the operator happened to be.
  test 'major takes the announcement file from the command line' do
    Tempfile.create(['announcement', '.md']) do |file|
      ARGV.replace([file.path])

      assert_equal File.expand_path(file.path), send(:major_notes)
    end
  end
end
