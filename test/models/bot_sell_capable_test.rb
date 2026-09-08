require 'test_helper'

# What decides whether the wash-sale question is put to the user at all: the bot they just armed has
# to be able to sell. Asked once per account, so a false negative here costs a user the protection
# silently — which is why every shape that can put a sell order on the book is covered.
class BotSellCapableTest < ActiveSupport::TestCase
  setup { @user = create(:user) }

  test 'a plain buying bot cannot sell' do
    assert_not_predicate create(:dca_single_asset, user: @user), :sell_capable?
  end

  test 'a bot already set to selling can' do
    bot = create(:dca_single_asset, user: @user)
    bot.settings = bot.settings.merge('direction' => 'selling')

    assert_predicate bot, :sell_capable?
  end

  test 'a buy-side trigger armed to flip into selling can, even while it is still buying' do
    bot = create(:dca_single_asset, user: @user)
    bot.settings = bot.settings.merge('price_limited' => true, 'price_limit' => 100,
                                      'price_limit_action' => 'start_selling')

    assert_predicate bot, :sell_capable?, 'the flip is armed; the sale is one price move away'
  end

  test 'the same trigger action on a switched-off trigger does not count' do
    bot = create(:dca_single_asset, user: @user)
    bot.settings = bot.settings.merge('price_limited' => false,
                                      'price_limit_action' => 'start_selling')

    assert_not_predicate bot, :sell_capable?
  end

  test 'a bot that pauses instead of flipping does not count' do
    bot = create(:dca_single_asset, user: @user)
    bot.settings = bot.settings.merge('price_limited' => true, 'price_limit' => 100,
                                      'price_limit_action' => 'pause')

    assert_not_predicate bot, :sell_capable?
  end

  test 'rebalancing sells to reach its targets, so it counts' do
    bot = create(:dca_index, user: @user)
    assert_not_predicate bot, :sell_capable?

    bot.settings = bot.settings.merge('rebalance_enabled' => true, 'rebalance_threshold' => 0.1)

    assert_predicate bot, :sell_capable?
  end

  test 'a composition bot holding a position counts — that position has a Sell button' do
    bot = create(:dca_index, user: @user)
    bot.stubs(:held_symbols).returns(['BTC'])

    assert_predicate bot, :sell_capable?
  end

  test 'a signal bot with an enabled sell rule counts' do
    bot = create(:signal_bot, user: @user)
    create(:bot_signal, bot: bot, direction: :buy)
    assert_not_predicate bot.reload, :sell_capable?

    create(:bot_signal, bot: bot, direction: :sell)

    assert_predicate bot.reload, :sell_capable?
  end

  test 'a switched-off sell rule does not count' do
    bot = create(:signal_bot, user: @user)
    create(:bot_signal, bot: bot, direction: :sell, enabled: false)

    assert_not_predicate bot.reload, :sell_capable?
  end
end
