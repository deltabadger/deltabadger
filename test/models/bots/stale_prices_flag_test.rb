require 'test_helper'

# When the live price read fails, every bot type falls back to valuing holdings at the last
# executed transaction price — and renders that as "Portfolio value" with nothing to say it is not
# live. With a rejected key the number simply stops moving, for weeks, while the page keeps
# presenting it as current. The fallback itself is right (a stale number beats a blank one); saying
# nothing about it is not.
class Bots::StalePricesFlagTest < ActiveSupport::TestCase
  FACTORIES = %i[dca_single_asset dca_index dca_multi_asset].freeze

  FACTORIES.each do |factory|
    test "#{factory}: metrics are marked stale when the price read fails" do
      bot = create(factory, user: create(:user))
      bot.stubs(:metrics).returns(minimal_metrics)
      bot.exchange.stubs(:get_tickers_prices).returns(Result::Failure.new('unauthorized.'))

      data = bot.metrics_with_current_prices(force: true)

      assert data[:prices_stale], 'a value carried over from the last trade must say so'
    end

    test "#{factory}: metrics are not marked stale when prices come back" do
      bot = create(factory, user: create(:user))
      bot.stubs(:metrics).returns(minimal_metrics)
      prices = bot.tickers.to_a.index_by(&:ticker).transform_values { 100.to_d }
      bot.exchange.stubs(:get_tickers_prices).returns(Result::Success.new(prices))

      data = bot.metrics_with_current_prices(force: true)

      assert_not data[:prices_stale]
    end
  end

  private

  # Only the keys the fallback path reads before returning: a non-empty chart (an empty one returns
  # earlier, before any price call) plus the accumulators each type touches on the way through.
  #
  # extra_series is one of them: the live point appends the holdings it was built from, so that
  # the chart can read holdings by index (see Bot::ChartSeries). Two rows covers single-asset
  # (net_base, realized); the index type reads it as a hash per point and appends its own.
  def minimal_metrics
    {
      chart: { labels: [Time.current], series: [[1.0], [1.0]], extra_series: [[1.0], [0.0]] },
      asset_breakdown: {},
      total_base_amount: 1.to_d,
      total_quote_amount_invested: 100.to_d,
      total_realized_proceeds: 0.to_d
    }
  end
end
