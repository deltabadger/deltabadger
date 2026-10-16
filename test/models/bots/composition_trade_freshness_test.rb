require 'test_helper'

# Trade paths value the basket from fresh data, never from the five-minute valuation cache (nor the
# thirty-day metrics cache under it). A fill lands, the metrics job is only enqueued, and a trade in
# the same window must already see it. The widget's drift reading stays cached.
class Bots::CompositionTradeFreshnessTest < ActiveSupport::TestCase
  def setup
    Rails.stubs(:cache).returns(ActiveSupport::Cache::MemoryStore.new)
    @bot = create(:dca_multi_asset, user: create(:user))
    @base0, @base1 = @bot.base_assets
    @tickers = @bot.composition_tickers
    @bot.instance_variable_set(:@tickers, @tickers)
    @bot.stubs(:composition_tickers).returns(@tickers)
    setup_bot_execution_mocks(@bot, price: 100)
    @bot.exchange.stubs(:get_tickers_prices).returns(Result::Success.new(@tickers.to_h { [it.ticker, 100.to_d] }))

    # base0 overweight in the warmed valuation; a later fill on base1 makes base1 the overweight one.
    fill(@base0, 2)
    fill(@base1, 1)
    @bot.metrics_with_current_prices
    fill(@base1, 2)
  end

  test 'the rebalance sell sizes against the fill that landed after the cache was warmed' do
    order = @bot.send(:rebalance_sell_order_data)

    assert_equal @base1.id, order[:ticker].base_asset_id
    assert_in_delta 0.5, order[:amount].to_f, 1e-9, '300 vs 200 at 50/50: 50 of base1 over target'
  end

  test 'the DCA-out sell ranks against the fill that landed after the cache was warmed' do
    orders = @bot.send(:get_sell_orders_data, 50.to_d).data

    assert_equal([@base1.id], orders.map { it[:ticker].base_asset_id })
  end

  test "the widget's drift reading stays on the cached valuation" do
    assert_in_delta 1 / 6r, @bot.rebalance_drift.to_f, 1e-9, 'the warmed 200/100 split, not the fresh 200/300'
  end

  private

  def fill(asset, amount)
    create(:transaction, bot: @bot, exchange: @bot.exchange, status: :submitted, external_status: :closed,
                         external_id: "t-#{SecureRandom.hex(4)}", side: :buy, base: asset.symbol,
                         quote: @bot.quote_asset.symbol, price: 100, amount:, amount_exec: amount,
                         quote_amount: amount * 100, quote_amount_exec: amount * 100)
  end
end
