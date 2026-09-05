require 'test_helper'

# A signal bot's first fill can be a sell — nothing forces a buy first. The metrics walk averages
# the buy price over buys only, and an average over nothing is NaN, which the page cannot round.
class Bots::SignalMetricsTest < ActiveSupport::TestCase
  test 'a sell-only history has no average buy price rather than NaN' do
    bot = create(:signal_bot, :started)
    create(:transaction, bot: bot, side: :sell, status: :submitted, external_status: :closed,
                         amount: 0.5, amount_exec: 0.5, price: 50_000, quote_amount: 25_000, quote_amount_exec: 25_000)

    metrics = bot.metrics(force: true)

    assert_nil metrics[:average_buy_price]
    assert_in_delta 25_000, metrics[:total_realized_proceeds].to_f, 1e-6
  end
end
