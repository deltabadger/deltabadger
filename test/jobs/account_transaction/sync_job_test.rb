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

  test 'records the sync error on the API key when the sync raises' do
    sync = mock('sync')
    sync.expects(:sync!).once.raises(StandardError, 'API error')
    AccountTransactionSync.expects(:new).with(@api_key).returns(sync)

    assert_raises(StandardError) { AccountTransaction::SyncJob.perform_now(@api_key) }

    assert_equal 'StandardError: API error', @api_key.reload.last_sync_error
  end

  test 'records the sync error on the API key when the sync returns a failure' do
    sync = mock('sync')
    sync.expects(:sync!).once.returns(Result::Failure.new('Invalid API-key'))
    AccountTransactionSync.expects(:new).with(@api_key).returns(sync)

    AccountTransaction::SyncJob.perform_now(@api_key)

    assert_equal 'Invalid API-key', @api_key.reload.last_sync_error
  end

  test 'matches transfers for the API key user after syncing' do
    withdrawal = AccountTransaction.create!(user: @user, exchange: @exchange, entry_type: :withdrawal,
                                            base_currency: 'BTC', base_amount: 1,
                                            transacted_at: Time.utc(2024, 5, 1), tx_id: 'sync-job-w')
    deposit = AccountTransaction.create!(user: @user, exchange: @exchange, entry_type: :deposit,
                                         base_currency: 'BTC', base_amount: '0.995'.to_d,
                                         transacted_at: Time.utc(2024, 5, 1, 1), tx_id: 'sync-job-d')
    sync = mock('sync')
    sync.expects(:sync!).once.returns(Result::Success.new(2))
    AccountTransactionSync.expects(:new).with(@api_key).returns(sync)

    AccountTransaction::SyncJob.perform_now(@api_key)

    assert_equal deposit.id, withdrawal.reload.linked_transaction_id
  end
end
