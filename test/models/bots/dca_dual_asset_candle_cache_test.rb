# frozen_string_literal: true

require 'test_helper'

# Regression: the dual-asset candle-merged metrics were cached for only 5.seconds (a
# commented-out debug leftover), so the chart re-fetched candles live on essentially
# every view. It must use the same ~5-minute cut as single-asset/index so a second view
# paints from cache.
#
# The test env uses :null_store, so we swap in a real MemoryStore and time-travel to
# observe TTL behaviour.
class DcaDualAssetCandleCacheTest < ActiveSupport::TestCase
  test 'candle-merged metrics persist well beyond 5 seconds' do
    store = ActiveSupport::Cache::MemoryStore.new
    Rails.stubs(:cache).returns(store)

    bot = create(:dca_dual_asset, user: create(:user))
    bot.stubs(:metrics_with_current_prices).returns(
      { chart: { labels: [Time.current], series: [[1.0], [1.0]] } }
    )
    # The expensive live candle fetch must happen ONCE, then be served from cache.
    bot.expects(:get_extended_chart_data_with_candles_data)
       .once
       .returns(Result::Success.new({ labels: [], series: [[], []] }))

    # Freeze on a 5-minute boundary so the cut-aligned TTL is deterministic (~5 min with
    # the fix, 5 s without). 30 s is unambiguously past the old debug TTL and well within
    # the fixed one, regardless of when the suite happens to run. (Sequential travel_to
    # calls, not nested blocks, which ActiveSupport disallows.)
    travel_to Time.utc(2026, 1, 1, 12, 0, 0)
    bot.metrics_with_current_prices_and_candles

    travel_to Time.utc(2026, 1, 1, 12, 0, 30)
    bot.metrics_with_current_prices_and_candles # served from cache; no second fetch
  ensure
    travel_back
  end
end
