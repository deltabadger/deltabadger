# frozen_string_literal: true

require 'test_helper'

# The recurring warm job keeps the current-price caches hot so the /bots index and the
# per-bot PnL broadcasts read warm caches instead of doing live exchange roundtrips.
#
# It refreshes metrics_with_current_prices(force: true) for measurable, non-deleted bots
# that HAVE submitted transactions (matching global_pnl's inclusion set), and pre-warms
# the FX rates those bots need. It does NOT warm the heavier candle path.
#
# Warming a cache nobody will read is a live exchange call for nothing, so the job only
# runs inside the activity window a signed-in request opens. The test env is :null_store,
# where that window can never open, so these tests swap in a real store.
class Bot::WarmMetricsCachesJobTest < ActiveJob::TestCase
  setup do
    @previous_cache = Rails.cache
    Rails.cache = ActiveSupport::Cache::MemoryStore.new
    Bot::WarmMetricsCachesJob.mark_activity!
  end

  teardown { Rails.cache = @previous_cache }

  test 'warms current-price metrics only for measurable bots with submitted transactions' do
    user = create(:user)
    exchange = create(:binance_exchange)
    btc = create(:asset, :bitcoin)
    usd = create(:asset, :usd)
    shared = { user: user, exchange: exchange, base_asset: btc, quote_asset: usd }

    bot_with_txns = create(:dca_single_asset, **shared)
    create(:transaction, bot: bot_with_txns)

    create(:dca_single_asset, **shared)            # no transactions -> skip
    deleted = create(:dca_single_asset, **shared)
    create(:transaction, bot: deleted)
    deleted.update!(status: :deleted)              # deleted -> skip

    # Exactly one bot qualifies -> exactly one forced price refresh.
    Bots::DcaSingleAsset.any_instance.expects(:metrics_with_current_prices).with(force: true).once

    Bot::WarmMetricsCachesJob.perform_now
  end

  test 'does not warm the candle path (price-only)' do
    user = create(:user)
    bot = create(:dca_single_asset, user: user)
    create(:transaction, bot: bot)

    Bots::DcaSingleAsset.any_instance.stubs(:metrics_with_current_prices)
    Bots::DcaSingleAsset.any_instance.expects(:metrics_with_current_prices_and_candles).never

    Bot::WarmMetricsCachesJob.perform_now
  end

  test 'warms multi-asset bots' do
    bot = create(:dca_multi_asset, user: create(:user))
    create(:transaction, bot: bot)

    Bots::DcaMultiAsset.any_instance.expects(:metrics_with_current_prices).with(force: true).once

    Bot::WarmMetricsCachesJob.perform_now
  end

  test 'pre-warms the FX rate a non-USD bot needs' do
    user = create(:user)
    eur = create(:asset, :eur)
    bot = create(:dca_single_asset, user: user, quote_asset: eur)
    create(:transaction, bot: bot, quote: 'EUR')

    Bots::DcaSingleAsset.any_instance.stubs(:metrics_with_current_prices)
    Utilities::Currency.expects(:exchange_rate).with(from: 'EUR', to: 'USD').at_least_once

    Bot::WarmMetricsCachesJob.perform_now
  end

  test 'does nothing when nobody has used the app recently' do
    user = create(:user)
    bot = create(:dca_single_asset, user: user)
    create(:transaction, bot: bot)

    Rails.cache.clear # no signed-in request has opened the window

    Bots::DcaSingleAsset.any_instance.expects(:metrics_with_current_prices).never
    Utilities::Currency.expects(:exchange_rate).never

    Bot::WarmMetricsCachesJob.perform_now
  end

  test 'the activity window a request opens expires' do
    user = create(:user)
    bot = create(:dca_single_asset, user: user)
    create(:transaction, bot: bot)

    Bots::DcaSingleAsset.any_instance.expects(:metrics_with_current_prices).never

    travel Bot::WarmMetricsCachesJob::ACTIVITY_WINDOW + 1.minute do
      Bot::WarmMetricsCachesJob.perform_now
    end
  end
end
