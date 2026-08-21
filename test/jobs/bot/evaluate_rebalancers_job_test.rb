require 'test_helper'

# The rebalance leg's only clock. Its selection rule IS the feature's headline: a stopped bot still
# rebalances, because the trigger is independent of the DCA schedule.
class Bot::EvaluateRebalancersJobTest < ActiveSupport::TestCase
  def setup
    @bot = create(:dca_dual_asset, user: create(:user))
    enable_rebalancing
  end

  test 'a stopped bot with rebalancing on is still evaluated' do
    @bot.update_columns(status: Bot.statuses[:stopped])

    Bot::RebalanceJob.expects(:perform_later).with(@bot)
    Bot::EvaluateRebalancersJob.perform_now
  end

  test 'a working bot is evaluated' do
    @bot.update_columns(status: Bot.statuses[:waiting])

    Bot::RebalanceJob.expects(:perform_later).with(@bot)
    Bot::EvaluateRebalancersJob.perform_now
  end

  test 'a deleted bot is not' do
    @bot.update_columns(status: Bot.statuses[:deleted])

    Bot::RebalanceJob.expects(:perform_later).never
    Bot::EvaluateRebalancersJob.perform_now
  end

  test 'an archived bot is not' do
    @bot.update_columns(status: Bot.statuses[:archived])

    Bot::RebalanceJob.expects(:perform_later).never
    Bot::EvaluateRebalancersJob.perform_now
  end

  test 'a bot with rebalancing switched off is not' do
    @bot.update_columns(settings: @bot.settings.merge('rebalance_enabled' => false))

    Bot::RebalanceJob.expects(:perform_later).never
    Bot::EvaluateRebalancersJob.perform_now
  end

  test 'a bot that never enabled rebalancing is not' do
    @bot.update_columns(settings: @bot.settings.except('rebalance_enabled', 'rebalance_threshold'))

    Bot::RebalanceJob.expects(:perform_later).never
    Bot::EvaluateRebalancersJob.perform_now
  end

  test 'a switched-off bot with an unfinished rebalance is still evaluated' do
    # The buy is owed. Dropping it from the fan-out would strand the sale proceeds forever.
    @bot.update_columns(settings: @bot.settings.merge('rebalance_enabled' => false))
    @bot.set_rebalance_pending!(phase: Bot::Rebalanceable::PHASE_BUYING, remaining_quote_amount: 20)

    Bot::RebalanceJob.expects(:perform_later).with(@bot)
    Bot::EvaluateRebalancersJob.perform_now
  end

  test 'index bots are evaluated too, stopped ones included' do
    index = create(:dca_index, user: create(:user))
    index.update_columns(
      settings: index.settings.merge('rebalance_enabled' => true, 'rebalance_threshold' => 0.05),
      status: Bot.statuses[:stopped]
    )
    @bot.update_columns(settings: @bot.settings.merge('rebalance_enabled' => false))

    Bot::RebalanceJob.expects(:perform_later).with(index)
    Bot::EvaluateRebalancersJob.perform_now
  end

  test 'multi-asset bots with rebalancing on are evaluated' do
    multi = create(:dca_multi_asset, user: create(:user), exchange: @bot.exchange,
                                     base_assets: [@bot.base0_asset, @bot.base1_asset],
                                     quote_asset: @bot.quote_asset)
    multi.update_columns(
      settings: multi.settings.merge('rebalance_enabled' => true, 'rebalance_threshold' => 0.05),
      status: Bot.statuses[:stopped]
    )
    @bot.update_columns(settings: @bot.settings.merge('rebalance_enabled' => false))

    Bot::RebalanceJob.expects(:perform_later).with(multi)
    Bot::EvaluateRebalancersJob.perform_now
  end

  test 'single-asset bots are never evaluated — they have no allocation to drift' do
    # Reuses the pair bot's assets — a fresh set would collide on external ids.
    single = create(:dca_single_asset, user: create(:user), exchange: @bot.exchange,
                                       base_asset: @bot.base0_asset, quote_asset: @bot.quote_asset)
    single.update_columns(settings: single.settings.merge('rebalance_enabled' => true))
    @bot.update_columns(settings: @bot.settings.merge('rebalance_enabled' => false))

    Bot::RebalanceJob.expects(:perform_later).never
    Bot::EvaluateRebalancersJob.perform_now
  end

  private

  def enable_rebalancing
    @bot.settings = @bot.settings.merge('rebalance_enabled' => true, 'rebalance_threshold' => 0.05)
    @bot.set_missed_quote_amount
    @bot.save!
  end
end
