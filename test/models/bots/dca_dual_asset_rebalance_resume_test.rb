require 'test_helper'

# The resumable half. A rebalance is two orders across a network, and between them the bot holds
# quote cash that #metrics does not model — so the drift reading in that window is wrong in a
# specific, expensive way: base0 has shrunk and the uncounted cash makes it look underweight, which
# an unguarded next poll would "fix" by selling base1. These pin the guards that stop that.
class Bots::DcaDualAssetRebalanceResumeTest < ActiveSupport::TestCase
  def setup
    @bot = create(:dca_dual_asset, user: create(:user))
    setup_bot_execution_mocks(@bot, price: 100)
    @bot.exchange.stubs(:market_sell).returns(Result::Success.new(order_id: 'sell-1'))
    # The refresh is exercised on its own below; elsewhere the transaction state is set directly.
    Bot::FetchAndUpdateOrderJob.stubs(:perform_now)
    enable_rebalancing
    stub_values(base0: 70, base1: 30)
  end

  test 'a pending rebalance blocks a new sell — the churn loop guard' do
    @bot.set_rebalance_pending!(phase: Bot::Rebalanceable::PHASE_SELLING, sell_transaction_id: sell_order.id)

    @bot.exchange.expects(:market_sell).never
    @bot.rebalance!
  end

  test 'an open sell waits instead of buying' do
    order = sell_order(external_status: :open)
    @bot.set_rebalance_pending!(phase: Bot::Rebalanceable::PHASE_SELLING, sell_transaction_id: order.id)

    @bot.exchange.expects(:market_buy).never
    @bot.rebalance!

    assert_equal Bot::Rebalanceable::PHASE_SELLING, @bot.reload.rebalance_pending[:phase]
  end

  test 'a closed sell hands its proceeds to the buy leg' do
    order = sell_order(external_status: :closed, quote_amount_exec: 20)
    @bot.set_rebalance_pending!(phase: Bot::Rebalanceable::PHASE_SELLING, sell_transaction_id: order.id)

    @bot.rebalance!

    buy = @bot.transactions.where(side: :buy).last
    assert_equal 'REBALANCE', buy.transaction_type
    assert_in_delta 20, buy.quote_amount.to_f, 0.01
    assert_equal @bot.base1_asset.symbol, buy.base, 'the proceeds go into the underweight asset'
  end

  test 'a cancelled sell with a partial fill still buys what it realized' do
    # Clearing here would strand real cash the bot cannot see, and hand the next poll a drift
    # reading that makes it sell the other asset.
    order = sell_order(external_status: :cancelled, quote_amount_exec: 15)
    @bot.set_rebalance_pending!(phase: Bot::Rebalanceable::PHASE_SELLING, sell_transaction_id: order.id)

    @bot.rebalance!

    assert_in_delta 15, @bot.transactions.where(side: :buy).last.quote_amount.to_f, 0.01
  end

  test 'a cancelled sell that filled nothing clears cleanly' do
    order = sell_order(external_status: :cancelled, quote_amount_exec: 0)
    @bot.set_rebalance_pending!(phase: Bot::Rebalanceable::PHASE_SELLING, sell_transaction_id: order.id)

    @bot.rebalance!

    assert_not @bot.reload.rebalance_pending?
  end

  test 'an abandoned sell halts as ambiguous — abandonment is not an accounting claim' do
    order = sell_order(external_status: :abandoned, quote_amount_exec: 20)
    @bot.set_rebalance_pending!(phase: Bot::Rebalanceable::PHASE_SELLING, sell_transaction_id: order.id)

    @bot.rebalance!

    assert @bot.reload.rebalance_ambiguous?
  end

  test 'an open buy keeps the state — cash committed is not cash converted' do
    buy = buy_order(external_status: :open)
    @bot.set_rebalance_pending!(phase: Bot::Rebalanceable::PHASE_BUYING, buy_transaction_id: buy.id,
                                remaining_quote_amount: 20)

    @bot.rebalance!

    assert_equal Bot::Rebalanceable::PHASE_BUYING, @bot.reload.rebalance_pending[:phase]
  end

  test 'a filled buy completes the rebalance' do
    buy = buy_order(external_status: :closed, quote_amount_exec: 20)
    @bot.set_rebalance_pending!(phase: Bot::Rebalanceable::PHASE_BUYING, buy_transaction_id: buy.id,
                                remaining_quote_amount: 20)

    @bot.rebalance!

    assert_not @bot.reload.rebalance_pending?
  end

  test 'a partially filled cancelled buy retries only the remainder' do
    buy = buy_order(external_status: :cancelled, quote_amount_exec: 12)
    @bot.set_rebalance_pending!(phase: Bot::Rebalanceable::PHASE_BUYING, buy_transaction_id: buy.id,
                                remaining_quote_amount: 40)

    @bot.rebalance!

    retried = @bot.transactions.where(side: :buy).where.not(id: buy.id).last
    assert_in_delta 28, retried.quote_amount.to_f, 0.01, 'only the 28 still owed, not the original 40'
  end

  test 'a remainder too small to trade is dust, logged once and cleared' do
    buy = buy_order(external_status: :cancelled, quote_amount_exec: 19)
    @bot.set_rebalance_pending!(phase: Bot::Rebalanceable::PHASE_BUYING, buy_transaction_id: buy.id,
                                remaining_quote_amount: 20)

    @bot.rebalance!

    assert_not @bot.reload.rebalance_pending?
    assert @bot.bot_activity_logs.exists?(event: 'rebalance_dust'),
           "the user's own realized cash left unconvertible is worth saying once"
  end

  test 'a resume survives the user switching rebalancing off mid-flight' do
    # Once the sell is placed the buy is owed. Gating the resume on the switch would strand the
    # proceeds as uninvested cash forever.
    order = sell_order(external_status: :closed, quote_amount_exec: 20)
    @bot.set_rebalance_pending!(phase: Bot::Rebalanceable::PHASE_SELLING, sell_transaction_id: order.id)
    update_settings(rebalance_enabled: false)

    @bot.rebalance!

    assert @bot.transactions.where(side: :buy).exists?, 'the buy is owed regardless of the switch'
  end

  test 'intent with no persisted order halts as ambiguous' do
    # A crash between writing intent and persisting the order: no read path can find an order whose
    # id we never stored, so guessing either way risks a double sell.
    @bot.set_rebalance_pending!(phase: Bot::Rebalanceable::PHASE_SELLING)

    @bot.rebalance!

    assert @bot.reload.rebalance_ambiguous?
  end

  test 'an ambiguous rebalance never places anything again' do
    @bot.set_rebalance_pending!(phase: Bot::Rebalanceable::PHASE_AMBIGUOUS)

    @bot.exchange.expects(:market_sell).never
    @bot.exchange.expects(:market_buy).never
    @bot.rebalance!
  end

  test 'a resume refreshes the order before reading it' do
    # FetchAndUpdateOrderJob is one-shot and a stopped bot runs no open-order sweep, so without this
    # nothing would ever advance the order and the rebalance would wait forever.
    Bot::FetchAndUpdateOrderJob.unstub(:perform_now)
    order = sell_order(external_status: :open)
    @bot.set_rebalance_pending!(phase: Bot::Rebalanceable::PHASE_SELLING, sell_transaction_id: order.id)

    Bot::FetchAndUpdateOrderJob.expects(:perform_now).with(order, update_missed_quote_amount: false)
    @bot.rebalance!
  end

  test 'the DCA leg stands down while a rebalance is pending' do
    # The per-exchange semaphore stops the two legs OVERLAPPING, not one following the other: a DCA
    # tick between the sell and the buy would spend the proceeds and leave the buy with no cash.
    @bot.set_rebalance_pending!(phase: Bot::Rebalanceable::PHASE_BUYING, remaining_quote_amount: 20)

    @bot.expects(:set_orders).never
    assert @bot.execute_action.success?
  end

  # == Findings from the Codex review of this branch ==

  test 'a buy that was attempted but left no order id halts instead of replaying' do
    # The worker can die after the venue accepted the buy but before its id reaches the state.
    # Replaying would spend the proceeds twice.
    @bot.set_rebalance_pending!(phase: Bot::Rebalanceable::PHASE_BUYING, remaining_quote_amount: 20,
                                buy_attempted: true)

    @bot.exchange.expects(:market_buy).never
    @bot.rebalance!

    assert @bot.reload.rebalance_ambiguous?
  end

  test 'a handoff that never reached the network is placed, not halted' do
    @bot.set_rebalance_pending!(phase: Bot::Rebalanceable::PHASE_BUYING, remaining_quote_amount: 20)

    @bot.rebalance!

    assert @bot.transactions.where(side: :buy).exists?
  end

  test 'a buy that cannot be priced keeps its state instead of being written off as dust' do
    # A failed price read is transient. Treating it as dust would clear an owed buy and let a later
    # poll sell again against holdings that were already corrected.
    @bot.set_rebalance_pending!(phase: Bot::Rebalanceable::PHASE_BUYING, remaining_quote_amount: 20)
    stub_ticker_ask_price_failure(@bot.ticker1)

    @bot.rebalance!

    assert_equal Bot::Rebalanceable::PHASE_BUYING, @bot.reload.rebalance_pending[:phase]
    assert_in_delta 20, @bot.rebalance_remaining_quote_amount.to_f, 0.01
  end

  test 'a closed sell with no quote figure yet waits rather than reading it as zero' do
    # Bot::FetchAndUpdateOrderJob explicitly allows a closed sell whose quote_amount_exec is still
    # nil. Reading that as "sold nothing" would clear the state and strand the proceeds.
    order = sell_order(external_status: :closed, quote_amount_exec: 20)
    order.update_columns(quote_amount_exec: nil, amount_exec: nil)
    @bot.set_rebalance_pending!(phase: Bot::Rebalanceable::PHASE_SELLING, sell_transaction_id: order.id)

    @bot.rebalance!

    assert_equal Bot::Rebalanceable::PHASE_SELLING, @bot.reload.rebalance_pending[:phase]
  end

  test 'proceeds fall back to the base fill at its price when the quote figure is missing' do
    order = sell_order(external_status: :closed, quote_amount_exec: 20)
    order.update_columns(quote_amount_exec: nil) # amount_exec 0.2 at price 100
    @bot.set_rebalance_pending!(phase: Bot::Rebalanceable::PHASE_SELLING, sell_transaction_id: order.id)

    @bot.rebalance!

    assert_in_delta 20, @bot.transactions.where(side: :buy).last.quote_amount.to_f, 0.01
  end

  test 'an unknown placement outcome halts, and a proven pre-transmission failure does not' do
    @bot.exchange.stubs(:market_sell).raises(Client::AmbiguousPlacementError, 'connection reset')

    @bot.rebalance!

    assert @bot.reload.rebalance_ambiguous?, 'the order may be live — never replay it'
  end

  test 'a pre-transmission network failure leaves nothing owed' do
    # "connection refused" is what Bot::ExchangeUser recognises as provably pre-transmission — the
    # connection was never accepted, so it re-raises instead of converting to an ambiguous placement.
    @bot.exchange.stubs(:market_sell).raises(Client::TransientNetworkError, 'connection refused: 1.2.3.4:8100')

    @bot.rebalance!

    assert_not @bot.reload.rebalance_pending?, 'nothing reached the venue, so nothing is owed'
  end

  private

  def update_settings(**attrs)
    @bot.settings = @bot.settings.merge(attrs.transform_keys(&:to_s))
    @bot.set_missed_quote_amount
    @bot.save!
  end

  def enable_rebalancing
    update_settings(rebalance_enabled: true, rebalance_threshold: 0.05)
  end

  def stub_ticker_ask_price_failure(ticker)
    ticker.stubs(:get_ask_price).returns(Result::Failure.new('price read failed'))
    ticker.stubs(:get_last_price).returns(Result::Failure.new('price read failed'))
  end

  def stub_values(base0:, base1:)
    @bot.stubs(:metrics_with_current_prices).returns(
      total_base0_amount_value_in_quote: base0.to_d,
      total_base1_amount_value_in_quote: base1.to_d,
      prices_stale: false
    )
  end

  def sell_order(external_status: :closed, quote_amount_exec: 20)
    create(:transaction, bot: @bot, exchange: @bot.exchange, status: :submitted,
                         external_status: external_status, external_id: "s-#{SecureRandom.hex(4)}",
                         side: :sell, transaction_type: 'REBALANCE',
                         base: @bot.base0_asset.symbol, quote: @bot.quote_asset.symbol,
                         price: 100, amount: 0.2, amount_exec: quote_amount_exec.to_d / 100,
                         quote_amount: 20, quote_amount_exec: quote_amount_exec)
  end

  def buy_order(external_status: :closed, quote_amount_exec: 20)
    create(:transaction, bot: @bot, exchange: @bot.exchange, status: :submitted,
                         external_status: external_status, external_id: "b-#{SecureRandom.hex(4)}",
                         side: :buy, transaction_type: 'REBALANCE',
                         base: @bot.base1_asset.symbol, quote: @bot.quote_asset.symbol,
                         price: 100, amount: 0.2, amount_exec: quote_amount_exec.to_d / 100,
                         quote_amount: 20, quote_amount_exec: quote_amount_exec)
  end
end
