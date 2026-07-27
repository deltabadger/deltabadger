require 'test_helper'

# A job queued before the venue was retired still deserializes and runs. None of these jobs
# degrades quietly on a failure Result — FetchAndUpdateOrderJob resolves only a :not_found result
# and raises on anything else — so each has to no-op up front instead of burning its retries and
# dead-lettering.
class RetiredExchangeJobsTest < ActiveSupport::TestCase
  setup do
    @retired = Exchanges::Bitmart.create!(name: 'Bitmart', available: false)
  end

  test 'FetchAndUpdateOrderJob no-ops for an order placed on a retired exchange' do
    bot = create(:dca_single_asset, exchange: @retired)
    order = create(:transaction, bot: bot, status: :submitted, external_status: :unknown,
                                 external_id: 'bm1')
    bot.expects(:get_order).never
    order.stubs(:bot).returns(bot)

    assert_nothing_raised { Bot::FetchAndUpdateOrderJob.new.perform(order) }
    assert_equal 'unknown', order.reload.external_status
  end

  # The regression Codex flagged: bot.exchange is mutable. Once the user moves a stranded Bitmart
  # bot to a live venue, a job still queued for the OLD order must not ask the NEW venue about a
  # Bitmart order id. transactions.exchange_id records where the order was actually placed.
  test 'FetchAndUpdateOrderJob keys off the order venue, not the bot current venue' do
    bot = create(:dca_single_asset, exchange: @retired)
    order = create(:transaction, bot: bot, exchange: @retired, status: :submitted,
                                 external_status: :unknown, external_id: 'bm2')

    binance = create(:binance_exchange)
    bot.update_column(:exchange_id, binance.id)
    bot.reload
    order.stubs(:bot).returns(bot)
    bot.expects(:get_order).never

    assert_nothing_raised { Bot::FetchAndUpdateOrderJob.new.perform(order) }
  end

  test 'FetchAndUpdateOpenOrdersJob no-ops for a bot on a retired exchange' do
    bot = create(:dca_single_asset, exchange: @retired)
    create(:transaction, bot: bot, status: :submitted, external_status: :open, external_id: 'bm3')
    bot.expects(:get_orders).never

    assert_nothing_raised { Bot::FetchAndUpdateOpenOrdersJob.new.perform(bot) }
  end

  test 'FetchAndCreateOrderJob no-ops for a bot on a retired exchange' do
    bot = create(:dca_single_asset, exchange: @retired)
    bot.expects(:get_order).never

    assert_nothing_raised { Bot::FetchAndCreateOrderJob.new.perform(bot, 'bm4') }
    assert_empty bot.transactions.where(external_id: 'bm4')
  end

  test 'a bot on a live exchange still fetches its order' do
    bot = create(:dca_single_asset, exchange: create(:binance_exchange))
    order = create(:transaction, bot: bot, status: :submitted, external_status: :unknown,
                                 external_id: 'live1', amount_exec: nil, quote_amount_exec: nil)
    order.stubs(:bot).returns(bot)
    bot.stubs(:get_order).returns(Result::Success.new(
                                    status: :closed, price: 50_000, amount: 0.002, quote_amount: 100,
                                    amount_exec: 0.002, quote_amount_exec: 100, ticker: bot.ticker,
                                    side: :buy, order_type: :market_order
                                  ))

    Bot::FetchAndUpdateOrderJob.new.perform(order)

    assert_equal 'closed', order.reload.external_status
  end
end
