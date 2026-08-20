require 'test_helper'

# Clearing a halted rebalance. The dangerous version of this endpoint is one that clears blindly:
# an ambiguous halt means an order MAY be live at the venue, and clearing on top of it lets the next
# poll place a second sell.
class Bots::RebalanceResolutionsControllerTest < ActionDispatch::IntegrationTest
  def setup
    create(:user, admin: true, setup_completed: true) # onboarding gate
    @user = create(:user)
    @bot = create(:dca_dual_asset, user: @user)
    sign_in @user
  end

  test 'clearing an ambiguous halt lets the bot rebalance again' do
    @bot.set_rebalance_pending!(phase: Bot::Rebalanceable::PHASE_AMBIGUOUS)

    post bot_rebalance_resolutions_path(bot_id: @bot.id)

    assert_not @bot.reload.rebalance_pending?
  end

  test 'the clear is recorded against the user who made it' do
    @bot.set_rebalance_pending!(phase: Bot::Rebalanceable::PHASE_AMBIGUOUS)

    post bot_rebalance_resolutions_path(bot_id: @bot.id)

    log = @bot.bot_activity_logs.find_by(event: 'rebalance_manually_resolved')
    assert log, 'a manual override of a safety halt has to leave a trace'
    assert_equal @user.id, log.details['user_id']
  end

  test 'a known order still open at the venue blocks the clear' do
    order = create(:transaction, bot: @bot, exchange: @bot.exchange, status: :submitted,
                                 external_status: :open, external_id: 'still-open',
                                 side: :sell, transaction_type: 'REBALANCE',
                                 base: @bot.base0_asset.symbol, quote: @bot.quote_asset.symbol,
                                 price: 100, amount: 0.2, quote_amount: 20)
    @bot.set_rebalance_pending!(phase: Bot::Rebalanceable::PHASE_AMBIGUOUS, sell_transaction_id: order.id)
    Bot::FetchAndUpdateOrderJob.stubs(:perform_now)

    post bot_rebalance_resolutions_path(bot_id: @bot.id)

    assert @bot.reload.rebalance_ambiguous?, 'clearing on top of a live order is how a double sell happens'
  end

  test 'a halt that still owes a buy hands the buy back instead of losing the ledger' do
    # Metrics cannot see the sale proceeds, so clearing outright would let the next poll read the
    # post-sell holdings as fresh drift and trade against money already committed.
    @bot.set_rebalance_pending!(phase: Bot::Rebalanceable::PHASE_AMBIGUOUS, remaining_quote_amount: 40)

    post bot_rebalance_resolutions_path(bot_id: @bot.id)

    pending = @bot.reload.rebalance_pending
    assert_equal Bot::Rebalanceable::PHASE_BUYING, pending[:phase]
    assert_in_delta 40, @bot.rebalance_remaining_quote_amount.to_f, 0.01
    assert_not pending[:buy_attempted], 'the retry is a fresh attempt'
  end

  test 'a live rebalance order the pending payload never referenced still blocks the clear' do
    # A worker can die after persist_accepted_order! but before the id reaches the state.
    create(:transaction, bot: @bot, exchange: @bot.exchange, status: :submitted,
                         external_status: :open, external_id: 'unlinked',
                         side: :sell, transaction_type: 'REBALANCE',
                         base: @bot.base0_asset.symbol, quote: @bot.quote_asset.symbol,
                         price: 100, amount: 0.2, quote_amount: 20)
    @bot.set_rebalance_pending!(phase: Bot::Rebalanceable::PHASE_AMBIGUOUS)
    Bot::FetchAndUpdateOrderJob.stubs(:perform_now)

    post bot_rebalance_resolutions_path(bot_id: @bot.id)

    assert @bot.reload.rebalance_ambiguous?
  end

  test "another user's bot is not resolvable" do
    @bot.set_rebalance_pending!(phase: Bot::Rebalanceable::PHASE_AMBIGUOUS)
    sign_out @user
    sign_in create(:user)

    post bot_rebalance_resolutions_path(bot_id: @bot.id)

    assert @bot.reload.rebalance_ambiguous?, 'a halt is only clearable by the bot owner'
  end

  test 'signing in is required' do
    sign_out @user
    @bot.set_rebalance_pending!(phase: Bot::Rebalanceable::PHASE_AMBIGUOUS)

    post bot_rebalance_resolutions_path(bot_id: @bot.id)

    assert @bot.reload.rebalance_pending?
  end
end
