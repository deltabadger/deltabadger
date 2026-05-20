require 'test_helper'

# After Finding 1, Bot::FetchAndCreateOrderJob is no longer enqueued by new code,
# but is kept as a compatibility shim so jobs already queued at deploy time still
# deserialize. Behavior: if a Transaction for the order_id already exists, delegate
# to FetchAndUpdateOrderJob; otherwise preserve the old fetch-then-create path.
class Bot::FetchAndCreateOrderJobTest < ActiveSupport::TestCase
  test 'delegates to FetchAndUpdateOrderJob when a transaction already exists' do
    bot = create(:dca_single_asset, :started)
    create(:transaction, bot: bot, status: :submitted, external_status: :unknown, external_id: 'u1')

    Bot::FetchAndUpdateOrderJob.expects(:perform_later).with(
      instance_of(Transaction),
      update_missed_quote_amount: true
    )

    Bot::FetchAndCreateOrderJob.new.perform(bot, 'u1', update_missed_quote_amount: true)
  end

  test 'falls back to fetch-then-create when no transaction exists yet' do
    bot = create(:dca_single_asset, :started)
    order_data = {
      status: :closed, price: 50_000, amount: 0.002, quote_amount: 100,
      amount_exec: 0.002, quote_amount_exec: 100,
      ticker: bot.ticker, side: :buy, order_type: :market_order, order_id: 'new1'
    }
    bot.stubs(:get_order).returns(Result::Success.new(order_data))

    assert_difference -> { bot.transactions.submitted.count }, 1 do
      Bot::FetchAndCreateOrderJob.new.perform(bot, 'new1')
    end

    assert_equal 'new1', bot.transactions.order(:created_at).last.external_id
  end
end
