require 'test_helper'

# The two pieces of state behind "stop a bot that cannot recover" and "mail about it at most once a
# day". They have deliberately different lifetimes and the tests are mostly about that difference:
# the strike is cleared by a successful run, the mail budget is not.
class Bot::FailableTest < ActiveSupport::TestCase
  setup do
    @bot = create(:dca_single_asset, :started)
  end

  test 'a blocking kind needs two in a row' do
    assert_not @bot.blocking_failure?(:invalid_key), 'one auth rejection is ambiguous on most venues'

    @bot.record_failure!(:invalid_key)

    assert @bot.blocking_failure?(:invalid_key)
  end

  test 'an intervening failure of another kind breaks the streak' do
    @bot.record_failure!(:invalid_key)
    @bot.record_failure!(:transient)

    assert_not @bot.blocking_failure?(:invalid_key),
               'two failures separated by an unrelated one are not consecutive'
  end

  test 'a successful run breaks the streak' do
    @bot.record_failure!(:restricted)
    @bot.clear_failure_state!

    assert_not @bot.blocking_failure?(:restricted)
  end

  test 'a recoverable kind never blocks, however many times it repeats' do
    3.times { @bot.record_failure!(:insufficient_funds) }

    assert_not @bot.blocking_failure?(:insufficient_funds)
    assert_not @bot.blocking_failure?(nil)
  end

  test 'the same kind is notified once a day, and again after it' do
    assert @bot.notify_about_failure?(:invalid_key)
    @bot.record_failure!(:invalid_key, notified: true)

    assert_not @bot.notify_about_failure?(:invalid_key)

    travel 25.hours do
      assert @bot.notify_about_failure?(:invalid_key)
    end
  end

  test 'a different kind is news and notifies immediately' do
    @bot.record_failure!(:invalid_key, notified: true)

    assert @bot.notify_about_failure?(:restricted)
  end

  # The shortcut that looks obvious and is wrong: clearing the budget on success means fail →
  # recover → fail the same way an hour later mails twice.
  test 'a successful run does not reopen the mail budget' do
    @bot.record_failure!(:transient, notified: true)
    @bot.clear_failure_state!

    assert_not @bot.notify_about_failure?(:transient)
  end

  test 'an unrecognised failure gets its own budget rather than sharing one with everything' do
    @bot.record_failure!(nil, notified: true)

    assert_not @bot.notify_about_failure?(nil)
    assert @bot.notify_about_failure?(:transient)
  end

  # merge_transient_data! exists because a read-modify-write here can erase the placement-intent
  # key, which is the only thing standing between an accepted-but-unrecorded order and a second one.
  test 'recording a failure leaves the rest of transient_data alone' do
    @bot.merge_transient_data!('rebalance_pending' => { 'id' => 7 })

    @bot.record_failure!(:invalid_key, notified: true)
    @bot.clear_failure_state!

    assert_equal({ 'id' => 7 }, @bot.reload.transient_data['rebalance_pending'])
  end

  test 'clearing an already-clean bot writes nothing' do
    @bot.expects(:merge_transient_data!).never

    @bot.clear_failure_state!
  end

  class BudgetSharingTest < ActiveSupport::TestCase
    # Buy-side out-of-funds is the one email Bot::Fundable already budgets, so this path shares that
    # column rather than opening a second day-long window on the same message.
    test 'a buying bot shares the end-of-funds budget' do
      bot = create(:dca_single_asset, :started)
      assert bot.notify_about_failure?(:insufficient_funds)

      bot.record_failure!(:insufficient_funds, notified: true)

      assert_not bot.notify_about_failure?(:insufficient_funds)
      assert bot.reload.last_end_of_funds_notification.present?
    end

    # ...but a selling bot spends BASE, its rejection goes out as notify_about_error, and Fundable's
    # window is scoped by QUOTE asset ACROSS the user's bots. Sharing it would let a buying bot's
    # low-USD warning swallow a selling bot's first insufficient-BTC alert.
    test 'a selling bot is not silenced by a sibling buying bot' do
      seller = selling_bot
      seller.user.bots.update_all(last_end_of_funds_notification: Time.current)

      assert seller.notify_about_failure?(:insufficient_funds)
    end

    test 'a selling bot does not spend the buying bots budget either' do
      seller = selling_bot

      seller.record_failure!(:insufficient_funds, notified: true)

      assert_nil seller.reload.last_end_of_funds_notification
      assert_not seller.notify_about_failure?(:insufficient_funds)
    end

    private

    def selling_bot
      bot = create(:dca_single_asset, :started)
      bot.settings = bot.settings.merge('direction' => 'selling')
      bot.set_missed_quote_amount
      bot.save!
      assert_predicate bot, :selling?
      bot
    end
  end
end
