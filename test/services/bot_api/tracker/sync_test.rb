# frozen_string_literal: true

require 'test_helper'

class BotApi::Tracker::SyncTest < ActiveSupport::TestCase
  setup { @user = create(:user) }

  test 'queues both syncs and names the exchanges' do
    api_key = create(:api_key, user: @user)
    AccountTransaction::SyncTrackerJob.expects(:perform_later).with(@user.id, [api_key.id])
    AccountBalance::SyncJob.expects(:perform_later).with(@user.id, [api_key.id])

    result = BotApi::Tracker::Sync.call(user: @user)

    assert result.success?, result.error_message
    assert_equal :accepted, result.status
    assert_equal ['Binance'], result.data[:exchanges]
  end

  test 'an account with nothing that can read history is refused' do
    AccountTransaction::SyncTrackerJob.expects(:perform_later).never

    result = BotApi::Tracker::Sync.call(user: @user)

    assert_equal 'no_reading_keys', result.error_code
    assert_equal :validation_failed, result.status
  end

  test 'a withdrawal-only key cannot read history either' do
    create(:api_key, user: @user, key_type: :withdrawal)
    AccountTransaction::SyncTrackerJob.expects(:perform_later).never

    assert_equal 'no_reading_keys', BotApi::Tracker::Sync.call(user: @user).error_code
  end
end
