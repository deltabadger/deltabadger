require 'test_helper'

class BroadcastsTest < ActionDispatch::IntegrationTest
  include ActiveJob::TestHelper

  setup do
    @user = create(:user, admin: true, setup_completed: true)
    sign_in @user
  end

  test 'wake_dispatcher dispatches overdue scheduled jobs' do
    post broadcasts_wake_dispatcher_path
    assert_response :ok
  end

  test 'wake_dispatcher succeeds with no overdue jobs' do
    post broadcasts_wake_dispatcher_path
    assert_response :ok
  end
end
