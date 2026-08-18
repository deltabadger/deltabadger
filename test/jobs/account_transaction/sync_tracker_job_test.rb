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

  test 'records the failure and names the failed exchange in a sync-warnings broadcast' do
    sync_binance = mock('sync_binance')
    sync_binance.expects(:sync!).once.raises(StandardError, 'API error')
    AccountTransactionSync.expects(:new).with(@api_key_binance).returns(sync_binance)

    sync_kraken = mock('sync_kraken')
    sync_kraken.expects(:sync!).once.returns(Result::Success.new(3))
    AccountTransactionSync.expects(:new).with(@api_key_kraken).returns(sync_kraken)

    Turbo::StreamsChannel.expects(:broadcast_remove_to).with("user_#{@user.id}", :sync, target: 'sync-progress')
    Turbo::StreamsChannel.expects(:broadcast_refresh_to).with("user_#{@user.id}", :sync)
    Turbo::StreamsChannel.expects(:broadcast_replace_to).with(
      "user_#{@user.id}", :sync,
      target: 'sync-warnings',
      partial: 'tracker/sync_warning',
      locals: { exchanges: ['Binance'] }
    )

    AccountTransaction::SyncTrackerJob.perform_now(@user.id, [@api_key_binance.id, @api_key_kraken.id])

    assert_equal 'StandardError: API error', @api_key_binance.reload.last_sync_error
    assert_nil @api_key_kraken.reload.last_sync_error
  end

  test 'a Result::Failure is recorded on the key and named in the warning broadcast' do
    sync_binance = mock('sync_binance')
    sync_binance.expects(:sync!).once.returns(Result::Failure.new('Invalid API-key'))
    AccountTransactionSync.expects(:new).with(@api_key_binance).returns(sync_binance)

    AccountTransaction::SyncTrackerJob.perform_now(@user.id, [@api_key_binance.id])

    assert_equal 'Invalid API-key', @api_key_binance.reload.last_sync_error
  end

  # Issue #153: the Kraken key that cannot read the ledger still trades, so the sync must record
  # the failure and explain it without condemning the key.
  test 'a permission failure is reported as such and leaves the key usable' do
    sync_kraken = mock('sync_kraken')
    sync_kraken.expects(:sync!).once.returns(Result::Failure.new('EGeneral:Permission denied'))
    AccountTransactionSync.expects(:new).with(@api_key_kraken).returns(sync_kraken)

    Turbo::StreamsChannel.expects(:broadcast_append_to).with(
      "user_#{@user.id}", :sync,
      target: 'flash',
      partial: 'tracker/sync_key_error',
      locals: { exchange_name: 'Kraken', message: 'EGeneral:Permission denied',
                reason: :permission, capability: :transactions }
    )

    AccountTransaction::SyncTrackerJob.perform_now(@user.id, [@api_key_kraken.id])

    assert_equal 'correct', @api_key_kraken.reload.status
    assert_equal 'EGeneral:Permission denied', @api_key_kraken.last_sync_error,
                 'the failure must still persist — TrackerController#index rebuilds the banner from it'
  end

  test 'matches transfers for the tracked user after syncing' do
    withdrawal = AccountTransaction.create!(user: @user, exchange: @binance, entry_type: :withdrawal,
                                            base_currency: 'BTC', base_amount: 1,
                                            transacted_at: Time.utc(2024, 5, 1), tx_id: 'tracker-job-w')
    deposit = AccountTransaction.create!(user: @user, exchange: @kraken, entry_type: :deposit,
                                         base_currency: 'BTC', base_amount: '0.995'.to_d,
                                         transacted_at: Time.utc(2024, 5, 1, 1), tx_id: 'tracker-job-d')
    sync_binance = mock('sync_binance')
    sync_binance.expects(:sync!).once.returns(Result::Success.new(1))
    AccountTransactionSync.expects(:new).with(@api_key_binance).returns(sync_binance)
    sync_kraken = mock('sync_kraken')
    sync_kraken.expects(:sync!).once.returns(Result::Success.new(1))
    AccountTransactionSync.expects(:new).with(@api_key_kraken).returns(sync_kraken)

    AccountTransaction::SyncTrackerJob.perform_now(@user.id, [@api_key_binance.id, @api_key_kraken.id])

    assert_equal deposit.id, withdrawal.reload.linked_transaction_id
  end
end
