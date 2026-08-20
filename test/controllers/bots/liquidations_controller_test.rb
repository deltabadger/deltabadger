require 'test_helper'

# Selling the assets an index has dropped, and clearing the halt when a sale's outcome is unknown.
class Bots::LiquidationsControllerTest < ActionDispatch::IntegrationTest
  def setup
    create(:user, admin: true, setup_completed: true) # onboarding gate
    @user = create(:user)
    @bot = create(:dca_index, user: @user)
    sign_in @user
  end

  test 'the sell is queued rather than run in the request' do
    post bot_liquidation_path(bot_id: @bot.id)

    assert_predicate queued(Bot::LiquidateExitedJob), :exists?
  end

  test 'who asked is recorded' do
    post bot_liquidation_path(bot_id: @bot.id)

    log = @bot.bot_activity_logs.find_by(event: 'liquidation_requested')
    assert log, 'a user-initiated sale has to leave a trace'
    assert_equal @user.id, log.details['user_id']
  end

  test 'a closed market says so now instead of failing quietly in a worker' do
    # A one-shot user command must not be silently dropped.
    @bot.exchange.class.any_instance.stubs(:market_open?).returns(false)

    post bot_liquidation_path(bot_id: @bot.id)

    assert_not_predicate queued(Bot::LiquidateExitedJob), :exists?
    assert_match(/closed/i, flash[:alert])
  end

  test 'a bot type that cannot have quitters is refused' do
    other = create(:dca_single_asset, user: @user)

    post bot_liquidation_path(bot_id: other.id)

    assert_not_predicate queued(Bot::LiquidateExitedJob), :exists?
  end

  test "another user's bot is not reachable" do
    stranger = create(:dca_single_asset, user: create(:user))

    post bot_liquidation_path(bot_id: stranger.id)

    assert_not_predicate queued(Bot::LiquidateExitedJob), :exists?
    assert_empty stranger.bot_activity_logs.where(event: 'liquidation_requested')
  end

  # == resolution ==

  test 'clearing a halt is queued behind the exchange semaphore, not done inline' do
    @bot.start_liquidation_placement!('CCC')
    @bot.flag_liquidation_ambiguous!

    post bot_liquidation_resolutions_path(bot_id: @bot.id, intent_id: @bot.liquidation_pending[:id])

    assert_predicate queued(Bot::ResolveLiquidationJob), :exists?
  end

  test 'a halt is not clearable while one of its orders is still working' do
    # The whole point of the halt is that an order MAY be live; clearing on top of one we can still
    # see would let the next attempt sell the same coins again.
    asset = create(:asset, symbol: 'CCC', name: 'Coin CCC', external_id: 'coin-ccc')
    create(:ticker, exchange: @bot.exchange, base_asset: asset, quote_asset: @bot.quote_asset)
    @bot.start_liquidation_placement!('CCC')
    @bot.flag_liquidation_ambiguous!
    create(:transaction, bot: @bot, exchange: @bot.exchange, status: :submitted, external_status: :open,
                         external_id: 'live-1', side: :sell, base: 'CCC', quote: @bot.quote_asset.symbol,
                         transaction_type: 'LIQUIDATION', price: 100, amount: 1)
    Bot::FetchAndUpdateOrderJob.any_instance.stubs(:perform)

    post bot_liquidation_resolutions_path(bot_id: @bot.id, intent_id: @bot.liquidation_pending[:id])

    assert_not_predicate queued(Bot::ResolveLiquidationJob), :exists?
    assert_match(/live-1/, flash[:alert])
  end

  private

  # Solid Queue is the adapter here, so ActiveJob's test-adapter assertions are unavailable.
  def queued(job_class)
    SolidQueue::Job.where(class_name: job_class.name)
  end
end
