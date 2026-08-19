# frozen_string_literal: true

require 'test_helper'

# One tile's PnL job must cost one bot, not the whole account.
#
# `broadcast_pnl_update` used to end with `user.broadcast_global_pnl_update`, and `User#global_pnl`
# walks every bot the user owns, computing `metrics_with_current_prices` for any whose cache is
# cold and finishing with two live FX conversions. The dashboard fires one of these jobs PER BOT,
# so N bots did N × N bots' worth of work — measured at 17.7s warm and 158s cold for fifteen bots,
# and it is the accounts with eighty bots that open that page.
#
# The global figure is owned by User::BroadcastGlobalPnlUpdateJob, which the page requests once.
class Bots::PnlBroadcastScopeTest < ActiveSupport::TestCase
  setup do
    @user = create(:user)
    @bot = create(:dca_single_asset, user: @user)
    @sibling = create(:dca_single_asset, user: @user, exchange: @bot.exchange,
                                         base_asset: @bot.base_asset, quote_asset: @bot.quote_asset)
  end

  test 'a tile broadcast never reaches another bot' do
    @bot.stubs(:metrics_with_current_prices).returns(@bot.metrics)
    @sibling.expects(:metrics_with_current_prices).never
    @bot.broadcast_pnl_update
  end

  test 'a tile broadcast does not recompute the account total' do
    @bot.stubs(:metrics_with_current_prices).returns(@bot.metrics)
    User.any_instance.expects(:global_pnl).never
    User.any_instance.expects(:broadcast_global_pnl_update).never

    @bot.broadcast_pnl_update
  end

  test 'the account total is still produced by its own job, without any tile job running' do
    User.any_instance.expects(:broadcast_global_pnl_update).once

    User::BroadcastGlobalPnlUpdateJob.perform_now(@user)
  end

  # A retry arriving while a pass is still running must not start a second full-account pass.
  test 'the global job is limited to one in flight per user' do
    job = User::BroadcastGlobalPnlUpdateJob

    assert_equal 1, job.concurrency_limit
    assert_equal :discard, job.concurrency_on_conflict
    assert_equal 'global_pnl_user_7', job.concurrency_key.call(User.new(id: 7))
  end
end
