require 'test_helper'
require 'open3'

class EngineLeaseTest < ActiveSupport::TestCase
  include ActiveJob::TestHelper

  def queue_adapter_for_test
    ActiveJob::QueueAdapters::TestAdapter.new
  end

  def lease!(value) = AppConfig.create!(key: EngineLease::KEY, value: value.to_json)

  test 'the lock initializer loads before every other initializer' do
    assert_equal '00_engine_lock.rb', Dir[Rails.root.join('config/initializers/*.rb')].map { |f| File.basename(f) }.min
  end

  test 'the lock sits next to the database this process actually opened' do
    opened = ActiveRecord::Base.connection_db_config.database
    assert_equal File.dirname(File.expand_path(opened, Rails.root)), EngineLease.primary_dir
  end

  test 'a URL override moves the lock with the database' do
    Dir.mktmpdir do |dir|
      out, status = Open3.capture2e({ 'PRIMARY_DATABASE_URL' => "sqlite3:#{dir}/elsewhere.sqlite3", 'RAILS_ENV' => 'test' },
                                    'bin/rails', 'runner', 'print EngineLease.primary_dir', chdir: Rails.root.to_s)
      assert status.success?, out
      assert_equal File.realpath(dir), File.realpath(out.lines.last.strip)
    end
  end

  test 'holds a shared lock that the rust engine cannot take, and fails while rust holds it' do
    Dir.mktmpdir do |dir|
      held = EngineLease.lock!(dir)
      File.open(File.join(dir, '.engine.lock'), File::RDONLY) do |rust|
        assert_not rust.flock(File::LOCK_EX | File::LOCK_NB), 'an exclusive lock is refused while Rails holds it'
        held.close
        assert rust.flock(File::LOCK_EX | File::LOCK_NB)
        assert_raises(EngineLease::HeldError) { EngineLease.lock!(dir) }
      end
    end
  end

  test 'refuses to start while the rust engine owns the install' do
    lease!(engine: 'rust', version: '0.1.0', since: Time.current.iso8601)
    error = assert_raises(EngineLease::HeldError) { EngineLease.check_handover! }
    assert_match 'deltabadger handback', error.message
  end

  test 'refuses to start on a handover row it cannot read' do
    envelope = '{"p":"AAAA","h":{"iv":"AAAAAAAAAAAAAAAA","at":"AAAAAAAAAAAAAAAAAAAAAA=="}}'
    AppConfig.connection.execute(
      "INSERT INTO app_configs (key, value, created_at, updated_at) VALUES ('#{EngineLease::KEY}', '#{envelope}', '2026-01-01', '2026-01-01')"
    )
    assert_raises(EngineLease::HeldError) { EngineLease.check_handover! }
  end

  test 'adopts bots handed back by the rust engine and clears the row' do
    lease!(engine: 'none', released_by: 'rust', handed_back: true, at: Time.current.iso8601)
    assert_enqueued_with(job: Bot::RepairOrphanedBotsJob) { EngineLease.check_handover! }
    assert_nil AppConfig.find_by(key: EngineLease::KEY)
  end

  test 'adoption polls outstanding orders, including on stopped bots' do
    bot = create(:dca_multi_asset, status: :stopped)
    open_order = create(:transaction, bot:, external_status: :open, external_id: 'OPEN-1', amount_exec: nil, quote_amount_exec: nil)
    create(:transaction, bot:, external_status: :closed, external_id: 'DONE-1')
    lease!(engine: 'none', released_by: 'rust', handed_back: true, at: Time.current.iso8601)
    EngineLease.check_handover!
    assert_enqueued_with(job: Bot::FetchAndUpdateOrderJob, args: [open_order, { update_missed_quote_amount: true }])
    assert_equal 1, enqueued_jobs.count { |j| j['job_class'] == 'Bot::FetchAndUpdateOrderJob' }, 'closed orders are not polled'
  end

  test 'a release that is not a completed handback is refused' do
    lease!(engine: 'none', released_by: 'rust', handed_back: false)
    assert_raises(EngineLease::HeldError) { EngineLease.check_handover! }
  end

  test 'does nothing without a row' do
    assert_no_enqueued_jobs { EngineLease.check_handover! }
  end
end
