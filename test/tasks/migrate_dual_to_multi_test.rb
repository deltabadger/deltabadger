require 'test_helper'
require 'rake'

# A bot mid-order or mid-rebalance at deploy time is skipped by the migration on purpose. This is
# how it gets converted afterwards, once it is quiet.
class MigrateDualToMultiTaskTest < ActiveSupport::TestCase
  setup do
    Bot::UpdateMetricsJob.stubs(:perform_later)
    Rails.application.load_tasks unless Rake::Task.task_defined?('bots:migrate_dual_to_multi')
    Rake::Task['bots:migrate_dual_to_multi'].reenable
    @bot = dual_asset_bot(status: :waiting)
  end

  def invoke! = capture_io { Rake::Task['bots:migrate_dual_to_multi'].invoke }

  # The shared :dca_dual_asset factory retired with the pair-only tests (Task 6) — a basket from the
  # surviving factory, re-typed and re-shaped in place, is the same fixture dual_to_composition_test.rb
  # builds for the class-already-gone case. Here the class still exists, so this reloads through it
  # directly rather than through DualToComposition::Row.
  def dual_asset_bot(status: :waiting)
    basket = create(:dca_multi_asset, status: status)
    base0, base1 = basket.base_asset_ids
    BotIndexAsset.where(bot_id: basket.id).delete_all
    settings = basket.settings.except('allocations', 'weighting')
                     .merge('base0_asset_id' => base0, 'base1_asset_id' => base1, 'allocation0' => 0.5)
    basket.update_columns(type: 'Bots::DcaDualAsset', settings: settings)
    Bots::DcaDualAsset.find(basket.id)
  end

  test 'it converts a bot the deploy-time migration skipped' do
    invoke!

    assert_equal 'Bots::DcaMultiAsset', Bot.find(@bot.id).type
  end

  test 'it still refuses a bot with a rebalance in flight' do
    @bot.update_columns(transient_data: @bot.transient_data.merge('rebalance_pending' => { 'phase' => 'sold' }))

    invoke!

    assert_equal 'Bots::DcaDualAsset', Bot.where(id: @bot.id).pick(:type)
  end

  test 'it is safe to run when there is nothing to do' do
    invoke!
    Rake::Task['bots:migrate_dual_to_multi'].reenable

    assert_nothing_raised { invoke! }
  end
end
