require 'test_helper'
require 'rake'

# A bot mid-order or mid-rebalance at deploy time is skipped by the migration on purpose. This is
# how it gets converted afterwards, once it is quiet.
class MigrateDualToMultiTaskTest < ActiveSupport::TestCase
  setup do
    Bot::UpdateMetricsJob.stubs(:perform_later)
    Rails.application.load_tasks unless Rake::Task.task_defined?('bots:migrate_dual_to_multi')
    Rake::Task['bots:migrate_dual_to_multi'].reenable
    @bot = create(:dca_dual_asset, status: :waiting)
  end

  def invoke! = Rake::Task['bots:migrate_dual_to_multi'].invoke

  test 'it converts a bot the deploy-time migration skipped' do
    invoke!

    assert_equal 'Bots::DcaMultiAsset', Bot.find(@bot.id).type
  end

  test 'it still refuses a bot with a rebalance in flight' do
    @bot.update_columns(transient_data: @bot.transient_data.merge('rebalance_pending' => { 'phase' => 'sold' }))

    invoke!

    assert_equal 'Bots::DcaDualAsset', Bot.where(id: @bot.id).pick(:type)
  end

  test 'it repoints a stray job left behind by a worker that finished after the migration' do
    Bot::DualToComposition.run!
    stray = SolidQueue::Job.create!(
      queue_name: 'default', class_name: 'Bot::ActionJob', priority: 0,
      arguments: { 'arguments' => [{ '_aj_globalid' => "gid://deltabadger/Bots::DcaDualAsset/#{@bot.id}" }] }
    )

    invoke!

    assert_includes stray.reload.arguments['arguments'].first['_aj_globalid'], 'Bots::DcaMultiAsset'
  end

  test 'it is safe to run when there is nothing to do' do
    invoke!
    Rake::Task['bots:migrate_dual_to_multi'].reenable

    assert_nothing_raised { invoke! }
  end
end
