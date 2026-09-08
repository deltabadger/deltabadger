require 'test_helper'

# A signal bot's buy arrives from an unauthenticated webhook: there is no user to show anything to
# and no schedule to carry money forward on, so a locked asset is skipped and recorded in the log
# alone, and the caller is answered exactly as before.
class Bots::SignalWashSaleTest < ActiveSupport::TestCase
  def setup
    @bot = create(:signal_bot, user: create(:user))
    @user = @bot.user
    @user.update!(wash_sale_enabled: true, wash_sale_jurisdiction: 'US')
  end

  def lock_it
    WashSaleLock.create!(user: @user, asset_id: @bot.ticker.base_asset_id, buy_locked_until: 10.days.from_now)
  end

  test 'a buy signal for a locked asset places nothing and never touches the venue' do
    lock_it
    signal = create(:bot_signal, bot: @bot, direction: :buy)
    # signal_order_data reads the venue and can record a failure the user is emailed about — the
    # guard sits in front of it, not after.
    @bot.expects(:signal_order_data).never
    @bot.expects(:place_signal_order).never

    @bot.send(:execute_signal, signal)

    assert_equal 0, @bot.transactions.count
    assert_empty @bot.bot_activity_logs.where("event LIKE '%wash_sale%'"), 'logger only'
  end

  test 'a SELL signal for a locked asset goes through' do
    lock_it
    signal = create(:bot_signal, :sell, bot: @bot)
    @bot.expects(:signal_order_data).once.returns(nil)

    @bot.send(:execute_signal, signal)
  end

  test 'an unlocked asset is unaffected' do
    signal = create(:bot_signal, bot: @bot, direction: :buy)
    @bot.expects(:signal_order_data).once.returns(nil)

    @bot.send(:execute_signal, signal)
  end

  test 'the rule being off lets the buy through' do
    lock_it
    @user.update!(wash_sale_enabled: false)
    signal = create(:bot_signal, bot: @bot, direction: :buy)
    @bot.expects(:signal_order_data).once.returns(nil)

    @bot.send(:execute_signal, signal)
  end
end
