# frozen_string_literal: true

require 'test_helper'

# Bot::WarmMetricsCachesJob only warms inside an activity window, and requests are what
# open it — without this the recurring job would never run again.
class WarmMetricsActivityWindowTest < ActionDispatch::IntegrationTest
  setup do
    create(:user, admin: true, setup_completed: true) # satisfies the onboarding gate
    @user = create(:user)
    @store = ActiveSupport::Cache::MemoryStore.new
    Rails.stubs(:cache).returns(@store)
  end

  test 'a signed-in request opens the window' do
    sign_in @user

    get bots_path

    assert_response :success
    assert Rails.cache.exist?(Bot::WarmMetricsCachesJob::ACTIVITY_KEY)
  end

  test 'an anonymous request does not open the window' do
    get new_user_session_path

    assert_response :success
    assert_not Rails.cache.exist?(Bot::WarmMetricsCachesJob::ACTIVITY_KEY)
  end
end
