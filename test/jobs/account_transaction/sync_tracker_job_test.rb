require 'test_helper'

class AccountTransaction::SyncTrackerJobTest < ActiveSupport::TestCase
  setup do
    @user = create(:user)
    @binance = create(:binance_exchange)
    @kraken = create(:kraken_exchange)
    @api_key_binance = create(:api_key, user: @user, exchange: @binance)
    @api_key_kraken = create(:api_key, user: @user, exchange: @kraken)
  end

  test 'syncs all provided API keys' do
    sync_binance = mock('sync_binance')
    sync_binance.expects(:sync!).once.returns(Result::Success.new(5))
    AccountTransactionSync.expects(:new).with(@api_key_binance).returns(sync_binance)

    sync_kraken = mock('sync_kraken')
    sync_kraken.expects(:sync!).once.returns(Result::Success.new(3))
    AccountTransactionSync.expects(:new).with(@api_key_kraken).returns(sync_kraken)

    AccountTransaction::SyncTrackerJob.perform_now(@user.id, [@api_key_binance.id, @api_key_kraken.id])
  end

  test 'skips failed exchange and continues to next' do
    sync_binance = mock('sync_binance')
    sync_binance.expects(:sync!).once.raises(StandardError, 'API error')
    AccountTransactionSync.expects(:new).with(@api_key_binance).returns(sync_binance)

    sync_kraken = mock('sync_kraken')
    sync_kraken.expects(:sync!).once.returns(Result::Success.new(3))
    AccountTransactionSync.expects(:new).with(@api_key_kraken).returns(sync_kraken)

    AccountTransaction::SyncTrackerJob.perform_now(@user.id, [@api_key_binance.id, @api_key_kraken.id])
  end
end
