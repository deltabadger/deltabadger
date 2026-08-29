require 'test_helper'

# No single pass can win the race against a live worker, so the conversion recurs until nothing is
# left. These pin that it keeps trying, keeps repairing, and costs nothing once it is done.
class Bot::ConvertDualAssetBotsJobTest < ActiveSupport::TestCase
  setup do
    Bot::UpdateMetricsJob.stubs(:perform_later)
    @bot = create(:dca_dual_asset, status: :waiting)
  end

  test 'it converts a pair bot' do
    Bot::ConvertDualAssetBotsJob.perform_now

    assert_equal 'Bots::DcaMultiAsset', Bot.find(@bot.id).type
  end

  test 'a bot it had to leave alone converts on the next pass' do
    @bot.update_columns(transient_data: @bot.transient_data.merge('rebalance_pending' => { 'phase' => 'sold' }))
    Bot::ConvertDualAssetBotsJob.perform_now
    assert_equal 'Bots::DcaDualAsset', Bot.where(id: @bot.id).pick(:type)

    # The rebalance settles; nobody has to remember to retry.
    @bot.update_columns(transient_data: @bot.transient_data.except('rebalance_pending'))
    Bot::ConvertDualAssetBotsJob.perform_now

    assert_equal 'Bots::DcaMultiAsset', Bot.find(@bot.id).type
  end

  test 'a later pass repairs a stale write that restored the pair shape' do
    base0 = @bot.base0_asset_id
    Bot::ConvertDualAssetBotsJob.perform_now
    # What a worker holding a pre-conversion instance does on save: settings by id, no type, and the
    # WHOLE settings hash — no allocations object. A merge that kept allocations would be the
    # harmless wizard-cookie case (see Bot::DualToComposition.clobbered), not this.
    Bot::DualToComposition::Row.where(id: @bot.id).update_all(
      settings: Bot.find(@bot.id).settings.merge('base0_asset_id' => base0,
                                                 'base1_asset_id' => @bot.base1_asset_id,
                                                 'allocation0' => 0.5).except('allocations')
    )

    Bot::ConvertDualAssetBotsJob.perform_now

    assert_not Bot.find(@bot.id).settings.key?('base0_asset_id')
  end

  test 'it does nothing at all once no pair bot is left' do
    Bot::ConvertDualAssetBotsJob.perform_now
    Bot::DualToComposition.expects(:run!).never

    Bot::ConvertDualAssetBotsJob.perform_now
  end
end
