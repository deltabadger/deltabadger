# frozen_string_literal: true

require 'test_helper'

# `Time.zone` is thread-local and outlives the request that set it. ActionMCP sets it to the
# calling user's zone (ActionMCP::Current#user=) and never puts it back, so one MCP call leaves
# every later request on that thread reading the clock in a stranger's zone — permanently, on a
# single-threaded server.
#
# Two things then go wrong, and both are covered here: a request's own `Date.current` moves, and
# the tracker ledger's cache key — built from a zone-aware timestamp — stops matching the key the
# warm-up job writes in the app zone, so the page never finds the ledger, enqueues the job again on
# every load, and refreshes itself in a loop.
class RequestTimeZoneTest < ActionDispatch::IntegrationTest
  include Devise::Test::IntegrationHelpers

  # +03:00, and one MCP call away from being this thread's zone.
  ELSEWHERE = 'Tallinn'

  setup do
    create(:user, admin: true, setup_completed: true) # platform requires an admin to exist
    @user = create(:user, setup_completed: true)
    @api_key = create(:api_key, user: @user)
    self.default_url_options = { locale: nil }
    sign_in @user
    Rails.stubs(:cache).returns(ActiveSupport::Cache::MemoryStore.new)
  end

  teardown { Time.zone = Time.zone_default }

  test 'a zone left on the thread does not follow the next request' do
    # 01:00 the next day in Tallinn, still the 1st here.
    travel_to Time.utc(2026, 8, 1, 22, 0) do
      Time.zone = ELSEWHERE

      get tracker_path

      assert_response :success
      assert_select 'input[name=?][value=?]', 'to', '2026-08-01'
    end
  end

  test 'a ledger warmed by the job is found by a request on a thread left in another zone' do
    create(:account_transaction, api_key: @api_key, transacted_at: Time.utc(2026, 8, 1, 12))
    Tracker::LedgerJob.perform_now(@user.id)
    Time.zone = ELSEWHERE

    with_test_adapter do
      assert_no_enqueued_jobs only: Tracker::LedgerJob do
        get tracker_path
      end
    end

    assert_response :success
  end

  private

  # The suite runs on the Solid Queue adapter, which has nothing to assert against.
  def with_test_adapter
    base_adapter = ActiveJob::Base.queue_adapter
    job_adapter = Tracker::LedgerJob.queue_adapter
    test_adapter = ActiveJob::QueueAdapters::TestAdapter.new
    ActiveJob::Base.queue_adapter = test_adapter
    Tracker::LedgerJob.queue_adapter = test_adapter
    yield
  ensure
    Tracker::LedgerJob.queue_adapter = job_adapter
    ActiveJob::Base.queue_adapter = base_adapter
  end
end
