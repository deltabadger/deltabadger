require 'test_helper'

class AccountTransaction::SyncJobTest < ActiveSupport::TestCase
  setup do
    @user = create(:user)
    @exchange = create(:binance_exchange)
    @api_key = create(:api_key, user: @user, exchange: @exchange)
  end

  test 'calls AccountTransactionSync#sync!' do
    sync = mock('sync')
    sync.expects(:sync!).once.returns(Result::Success.new(0))
    AccountTransactionSync.expects(:new).with(@api_key).returns(sync)

    AccountTransaction::SyncJob.perform_now(@api_key)
  end
end
