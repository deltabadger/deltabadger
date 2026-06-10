# frozen_string_literal: true

require 'test_helper'

# The balances table needs only the batched price fetch (~300 ms); the chart needs the
# expensive per-asset candle fetches. broadcast_metrics_update must therefore broadcast
# the metrics partial BEFORE candles are computed, so balances never wait for the chart.
class TwoStageMetricsBroadcastTest < ActiveSupport::TestCase
  FACTORIES = {
    dca_single_asset: 'bots/dca_single_assets/metrics',
    dca_dual_asset: 'bots/dca_dual_assets/metrics',
    dca_index: 'bots/dca_indexes/metrics'
  }.freeze

  FACTORIES.each do |factory, metrics_partial|
    test "#{factory}: metrics partial is broadcast before candles are computed" do
      bot = create(factory, user: create(:user))
      prices_data = { chart: { labels: [Time.current], series: [[1.0], [1.0]] } }
      combined_data = { chart: { labels: [Time.current], series: [[2.0], [2.0]] } }

      seq = sequence('two_stage')
      bot.stubs(:metrics_with_current_prices).returns(prices_data)

      bot.expects(:broadcast_replace_to)
         .with(anything, has_entries(target: 'metrics', partial: metrics_partial,
                                     locals: has_entries(metrics: prices_data, loading: false)))
         .in_sequence(seq)
      bot.expects(:metrics_with_current_prices_and_candles)
         .returns(combined_data)
         .in_sequence(seq)
      bot.expects(:broadcast_replace_to)
         .with(anything, has_entries(target: 'chart', partial: 'bots/chart',
                                     locals: has_entries(metrics: combined_data, loading: false)))
         .in_sequence(seq)

      bot.broadcast_metrics_update
    end
  end
end
