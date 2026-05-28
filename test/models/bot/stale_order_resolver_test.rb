require 'test_helper'

class Bot::StaleOrderResolverTest < ActiveSupport::TestCase
  # The resolver decides what to do with a Transaction whose external order
  # the exchange no longer tracks. Young orders are likely real bugs (wrong key,
  # subaccount mismatch); old orders are almost certainly stale and should be
  # marked abandoned so the poller stops hammering them.

  test 'returns :too_young and does not mutate the order when below the threshold' do
    bot = create(:dca_single_asset)
    order = create(:transaction, bot: bot, status: :submitted, external_status: :unknown,
                                 external_id: 'TXID-YOUNG', created_at: 1.day.ago)

    outcome = Bot::StaleOrderResolver.resolve(order)

    assert_equal :too_young, outcome
    assert_equal 'unknown', order.reload.external_status
  end

  test 'returns :abandoned and flips external_status when older than the threshold' do
    bot = create(:dca_single_asset)
    order = create(:transaction, bot: bot, status: :submitted, external_status: :unknown,
                                 external_id: 'TXID-STALE',
                                 created_at: (Bot::StaleOrderResolver::STALE_ORDER_THRESHOLD + 1.day).ago)

    outcome = Bot::StaleOrderResolver.resolve(order)

    assert_equal :abandoned, outcome
    assert_equal 'abandoned', order.reload.external_status
  end

  test 'STALE_ORDER_THRESHOLD is configured to 14 days' do
    # Locks the heuristic for the polling spam fix: matches Kraken's documented
    # QueryOrders retention window for closed orders.
    assert_equal 14.days, Bot::StaleOrderResolver::STALE_ORDER_THRESHOLD
  end
end
