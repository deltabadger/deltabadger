require 'test_helper'

# A rebalance spends quote the bot already owned — it swaps assets, it does not contribute. If
# contribution accounting counted it, a rebalance buy would satisfy a scheduled contribution the
# user never made, and would eat the "don't spend more than N" cap with money that was never new.
class Bot::RebalanceAccountingIsolationTest < ActiveSupport::TestCase
  def setup
    @bot = create(:dca_dual_asset, :started, user: create(:user))
    @bot.set_missed_quote_amount
    @bot.save!
  end

  test 'a rebalance buy does not count toward the contribution carry' do
    baseline = @bot.pending_quote_amount

    create_buy(transaction_type: 'REBALANCE', quote_amount_exec: 100)

    assert_equal baseline, @bot.reload.pending_quote_amount
  end

  test 'a regular buy still counts toward the contribution carry' do
    baseline = @bot.pending_quote_amount

    create_buy(transaction_type: 'REGULAR', quote_amount_exec: 100)

    assert_operator @bot.reload.pending_quote_amount, :<, baseline,
                    'a real contribution must still reduce what is owed this interval'
  end

  test 'a rebalance buy does not consume the spend cap' do
    enable_spend_limit(500)
    create_buy(transaction_type: 'REBALANCE', quote_amount_exec: 100)

    assert_equal 500, @bot.reload.quote_amount_available_before_limit_reached
  end

  test 'a regular buy still consumes the spend cap' do
    enable_spend_limit(500)
    create_buy(transaction_type: 'REGULAR', quote_amount_exec: 100)

    assert_equal 400, @bot.reload.quote_amount_available_before_limit_reached
  end

  test "the DCA panel's last order ignores rebalance fills" do
    regular = create_buy(transaction_type: 'REGULAR', quote_amount_exec: 100)
    create_buy(transaction_type: 'REBALANCE', quote_amount_exec: 100)

    assert_equal regular.id, @bot.reload.last_transaction.id
  end

  test 'the order-fetch job does not draw the carry down for a rebalance fill' do
    # Bot::LimitOrderable#execute_action sweeps every waiting order with update_missed_quote_amount
    # set, before any bot-level guard runs — so the exclusion has to live in the job itself.
    order = create(:transaction, bot: @bot, exchange: @bot.exchange, status: :submitted,
                                 external_status: :open, external_id: 'reb-open',
                                 side: :buy, transaction_type: 'REBALANCE',
                                 base: @bot.base0_asset.symbol, quote: @bot.quote_asset.symbol,
                                 price: 100, amount: 1, quote_amount: 100)
    @bot.exchange.stubs(:get_order).returns(Result::Success.new(
                                              status: :closed, order_id: 'reb-open', price: 100,
                                              amount_exec: 1, quote_amount_exec: 100
                                            ))
    carry_before = @bot.missed_quote_amount

    Bot::FetchAndUpdateOrderJob.perform_now(order, update_missed_quote_amount: true)

    assert_equal carry_before, @bot.reload.missed_quote_amount
  end

  private

  def enable_spend_limit(limit)
    @bot.settings = @bot.settings.merge('quote_amount_limited' => true, 'quote_amount_limit' => limit)
    @bot.set_missed_quote_amount
    @bot.save!
    @bot.reload
  end

  def create_buy(transaction_type:, quote_amount_exec:)
    create(:transaction, bot: @bot, exchange: @bot.exchange, status: :submitted,
                         external_status: :closed, external_id: "x-#{SecureRandom.hex(4)}",
                         side: :buy, transaction_type: transaction_type,
                         base: @bot.base0_asset.symbol, quote: @bot.quote_asset.symbol,
                         price: 100, amount: quote_amount_exec.to_d / 100,
                         amount_exec: quote_amount_exec.to_d / 100,
                         quote_amount: quote_amount_exec, quote_amount_exec: quote_amount_exec)
  end
end
