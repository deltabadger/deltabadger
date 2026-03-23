require 'test_helper'

class AccountTransaction::SyncAllJobTest < ActiveSupport::TestCase
  test 'enqueues sync jobs for each correct trading API key' do
    user = create(:user)
    exchange = create(:binance_exchange)
    correct_key = create(:api_key, user: user, exchange: exchange, status: :correct)

    other_exchange = create(:kraken_exchange)
    _incorrect_key = create(:api_key, :incorrect, user: user, exchange: other_exchange)

    AccountTransaction::SyncJob.expects(:set).with(wait: 0.seconds).returns(AccountTransaction::SyncJob)
    AccountTransaction::SyncJob.expects(:perform_later).with(correct_key).once

    AccountTransaction::SyncAllJob.perform_now
  end
end
