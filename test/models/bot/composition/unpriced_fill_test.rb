require 'test_helper'

# A fill is known when its executed amount and executed value are known, whatever the row's price says:
# Alpaca records a NULL price on an order it never echoed one for, and a cancelled partial reports no
# executed value until polled again. Dropping such a fill makes the basket read its asset as
# underweight and buy it again.
class Bot::Composition::UnpricedFillTest < ActiveSupport::TestCase
  setup do
    @exchange = create(:alpaca_exchange)
    @usd = Asset.find_by(symbol: 'USD') || create(:asset, :usd)
    @aaa = create(:asset, external_id: 'aaa', symbol: 'AAA')
    @bbb = create(:asset, external_id: 'bbb', symbol: 'BBB')
    @bot = create(:dca_multi_asset, exchange: @exchange, with_api_key: false, base_assets: [@aaa, @bbb], quote_asset: @usd)
    Ticker.any_instance.stubs(:get_ask_price).returns(Result::Success.new(10.to_d))
    Ticker.any_instance.stubs(:get_last_price).returns(Result::Success.new(10.to_d))
  end

  def order(symbol, side: :buy, status: :closed, price: 10, amount: 5, amount_exec: 5, quote_amount_exec: 50, at: 3.days.ago)
    create(:transaction, bot: @bot, exchange: @exchange, base: symbol, quote: 'USD', side:, external_status: status,
                         price:, amount:, amount_exec:, quote_amount: amount * 10, quote_amount_exec:, created_at: at)
  end

  def next_buys
    result = @bot.send(:get_orders_data, 100)
    assert_predicate result, :success?
    result.data.to_h { |o| [o[:ticker].base, o[:quote_amount].to_d] }
  end

  def holdings(metrics) = metrics[:asset_breakdown].transform_values { |row| row[:amount].to_d }.select { |_, v| v.positive? }

  test 'a closed buy with no price counts at its executed amount and value' do
    order('AAA', price: nil)
    order('BBB')
    order('AAA', side: :sell, amount: 2, amount_exec: 2, quote_amount_exec: 20, at: 2.days.ago)

    metrics = @bot.metrics(force: true)

    assert_equal({ 'AAA' => 3, 'BBB' => 5 }, holdings(metrics))
    assert_equal 100, metrics[:total_quote_amount_invested].to_d
    assert_equal 20, metrics[:rebalance_cash].to_d
    assert_equal({ 'AAA' => 60, 'BBB' => 40 }, next_buys.transform_values(&:round))
    assert_includes @bot.chart_buy_marks.map(&:second), 'AAA'
  end

  test 'a cancelled partial with no executed value counts at its price times what it executed' do
    order('AAA', status: :cancelled, amount: 10, quote_amount_exec: nil)
    order('BBB')
    order('AAA', side: :sell, amount: 2, amount_exec: 2, quote_amount_exec: 20, at: 2.days.ago)

    metrics = @bot.metrics(force: true)

    assert_equal({ 'AAA' => 3, 'BBB' => 5 }, holdings(metrics))
    assert_equal 100, metrics[:total_quote_amount_invested].to_d
    assert_equal({ 'AAA' => 60, 'BBB' => 40 }, next_buys.transform_values(&:round))
  end

  test 'a fill with no price, no executed amount and no executed value is never guessed' do
    order('AAA', price: nil, amount_exec: nil, quote_amount_exec: nil)
    order('BBB')

    metrics = @bot.metrics(force: true)

    # Skipped by the performance walk as before: no holding, no contribution. Its units still open a tax
    # lot (a closed order filled its requested amount), at an unknown cost.
    assert_equal({ 'BBB' => 5 }, holdings(metrics))
    assert_equal 50, metrics[:total_quote_amount_invested].to_d
    assert_equal [{ amount: 5.to_d, cost: nil }], metrics[:asset_lots]['AAA']
  end
end
