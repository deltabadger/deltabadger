require 'test_helper'

class AccountTransaction::SyncJobConcurrencyTest < ActiveSupport::TestCase
  # `limits_concurrency` here is `to: 1, on_conflict: :discard`. A key that names only the venue
  # therefore does not DELAY a second user's sync — it throws it away, and that user's ledger simply
  # never catches up.
  test 'two users syncing the same venue do not discard each other' do
    exchange = create(:binance_exchange)
    one = create(:api_key, user: create(:user), exchange: exchange)
    two = create(:api_key, user: create(:user), exchange: exchange)

    # concurrency_key, not a helper: Solid Queue evaluates the lambda on the job INSTANCE.
    key = AccountTransaction::SyncJob.new(one).concurrency_key

    assert_not_equal key, AccountTransaction::SyncJob.new(two).concurrency_key
    assert_includes key, one.user_id.to_s
  end

  test 'one user syncing two venues still serialises per venue' do
    user = create(:user)
    binance = create(:api_key, user: user, exchange: create(:binance_exchange))
    kraken = create(:api_key, user: user, exchange: create(:kraken_exchange))

    assert_not_equal AccountTransaction::SyncJob.new(binance).concurrency_key,
                     AccountTransaction::SyncJob.new(kraken).concurrency_key
  end

  test 'the same user and venue still collapses to one' do
    api_key = create(:api_key, user: create(:user), exchange: create(:binance_exchange))

    assert_equal AccountTransaction::SyncJob.new(api_key).concurrency_key,
                 AccountTransaction::SyncJob.new(api_key).concurrency_key
  end
end
