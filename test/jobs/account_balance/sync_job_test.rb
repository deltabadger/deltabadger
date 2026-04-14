require 'test_helper'

class AccountBalance::SyncJobTest < ActiveSupport::TestCase
  setup do
    @user = create(:user)
    @binance = create(:binance_exchange)
    @kraken = create(:kraken_exchange)
    @key_binance = create(:api_key, user: @user, exchange: @binance)
    @key_kraken = create(:api_key, user: @user, exchange: @kraken)
  end

  def ok_summary(priced_fresh: 2, unpriced: 0, pricing_error: nil)
    Result::Success.new(AccountBalance::Sync::Summary.new(
                          synced: priced_fresh + unpriced, priced_fresh: priced_fresh,
                          priced_stale: 0, unpriced: unpriced, pricing_error: pricing_error
                        ))
  end

  test 'calls Sync for each provided valid trading api key' do
    sync_b = mock
    sync_b.expects(:sync!).once.returns(ok_summary)
    AccountBalance::Sync.expects(:new).with(@key_binance).returns(sync_b)

    sync_k = mock
    sync_k.expects(:sync!).once.returns(ok_summary(priced_fresh: 1))
    AccountBalance::Sync.expects(:new).with(@key_kraken).returns(sync_k)

    AccountBalance::SyncJob.perform_now(@user.id, [@key_binance.id, @key_kraken.id])
  end

  test 'broadcasts pricing warning flash when pricing fully fails' do
    sync_b = mock
    sync_b.expects(:sync!).once.returns(ok_summary(priced_fresh: 0, unpriced: 2, pricing_error: 'CG down'))
    AccountBalance::Sync.expects(:new).with(@key_binance).returns(sync_b)

    Turbo::StreamsChannel.expects(:broadcast_append_to).with(
      "user_#{@user.id}", :sync,
      has_entries(target: 'flash', partial: 'tracker/pricing_warning')
    )

    AccountBalance::SyncJob.perform_now(@user.id, [@key_binance.id])
  end

  test 'skips non-trading or incorrect keys' do
    @key_kraken.update!(status: :incorrect)
    AccountBalance::Sync.expects(:new).never

    AccountBalance::SyncJob.perform_now(@user.id, [@key_kraken.id])
  end

  test 'flips key status to incorrect and broadcasts flash on invalid-key failure' do
    sync_b = mock
    sync_b.expects(:sync!).once.returns(Result::Failure.new('API-key format invalid.'))
    AccountBalance::Sync.expects(:new).with(@key_binance).returns(sync_b)

    Turbo::StreamsChannel.expects(:broadcast_append_to).with(
      "user_#{@user.id}", :sync,
      has_entries(target: 'flash', partial: 'tracker/sync_key_error')
    )

    AccountBalance::SyncJob.perform_now(@user.id, [@key_binance.id])

    assert_equal 'incorrect', @key_binance.reload.status
  end

  test 'broadcasts flash but does not flip status for non-auth failures' do
    sync_b = mock
    sync_b.expects(:sync!).once.returns(Result::Failure.new('Rate limited'))
    AccountBalance::Sync.expects(:new).with(@key_binance).returns(sync_b)

    Turbo::StreamsChannel.expects(:broadcast_append_to)

    AccountBalance::SyncJob.perform_now(@user.id, [@key_binance.id])

    assert_equal 'correct', @key_binance.reload.status
  end

  test 'continues when one exchange raises' do
    sync_b = mock
    sync_b.expects(:sync!).once.raises(StandardError, 'boom')
    AccountBalance::Sync.expects(:new).with(@key_binance).returns(sync_b)

    sync_k = mock
    sync_k.expects(:sync!).once.returns(Result::Success.new(1))
    AccountBalance::Sync.expects(:new).with(@key_kraken).returns(sync_k)

    AccountBalance::SyncJob.perform_now(@user.id, [@key_binance.id, @key_kraken.id])
  end
end
