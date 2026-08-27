# frozen_string_literal: true

require 'test_helper'

# The marks under the chart have to name the same fills the curve above them is drawn from.
# A row that carries a price but never executed — an order still open, or cancelled before it
# filled — would otherwise put a logo under a purchase that never happened.
class Bot::ChartBuyMarksTest < ActiveSupport::TestCase
  # Lazy, not a `setup`: the two-asset test below builds its own bot, and a bitcoin asset created
  # eagerly here would collide with the one that factory creates.
  def bot
    @bot ||= create(:dca_single_asset, :started)
  end

  def mark_symbols
    bot.chart_buy_marks.map(&:last)
  end

  test 'an executed buy is marked' do
    create(:transaction, bot: bot, base: 'BTC')

    assert_equal ['BTC'], mark_symbols
  end

  test 'a sell is not a buy' do
    create(:transaction, bot: bot, base: 'BTC', side: :sell)

    assert_empty mark_symbols
  end

  test 'an order still open with nothing executed is not marked' do
    create(:transaction, bot: bot, base: 'BTC', external_status: :open,
                         amount_exec: nil, quote_amount_exec: nil)

    assert_empty mark_symbols
  end

  test 'an order cancelled before it filled is not marked' do
    create(:transaction, bot: bot, base: 'BTC', external_status: :cancelled,
                         amount_exec: nil, quote_amount_exec: nil)

    assert_empty mark_symbols
  end

  test 'an order cancelled after a partial fill is marked' do
    create(:transaction, bot: bot, base: 'BTC', external_status: :cancelled,
                         amount_exec: 0.0004, quote_amount_exec: 20)

    assert_equal ['BTC'], mark_symbols
  end

  # `confirmed_exec_amounts` fills a closed row's execution in from amount * price, and the
  # metrics count it — so the mark has to count it too.
  test 'a closed order whose execution was never written back is marked' do
    create(:transaction, bot: bot, base: 'BTC', external_status: :closed,
                         amount_exec: nil, quote_amount_exec: nil)

    assert_equal ['BTC'], mark_symbols
  end

  test 'a zero fill is not a fill' do
    create(:transaction, bot: bot, base: 'BTC', amount_exec: 0, quote_amount_exec: 0)

    assert_empty mark_symbols
  end

  test 'a row with no price at all is not marked' do
    create(:transaction, bot: bot, base: 'BTC', price: nil,
                         amount_exec: nil, quote_amount_exec: nil)

    assert_empty mark_symbols
  end

  test 'a failed attempt is not marked' do
    create(:transaction, bot: bot, base: 'BTC', status: :failed)

    assert_empty mark_symbols
  end

  # A two-asset bot, so the pairing of a time with its own symbol is actually visible.
  test 'marks arrive oldest first, each with its own time' do
    bot = create(:dca_dual_asset, :started)
    create(:transaction, bot: bot, base: 'BTC', created_at: 2.days.ago)
    create(:transaction, bot: bot, base: 'ETH', created_at: 1.day.ago)

    times, symbols = bot.chart_buy_marks.transpose

    assert_equal %w[BTC ETH], symbols
    assert times[0] < times[1]
  end
end
