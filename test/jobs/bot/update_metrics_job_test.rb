# frozen_string_literal: true

require 'test_helper'

# UpdateMetricsJob runs after transaction-affecting changes (custom amount edits,
# exchange change, imports). Forcing only the base metrics left the 5-minute
# metrics_with_current_prices cache stale — and both the show page (which renders
# balances from that cache) and the subsequent broadcast read it with force: false,
# so users saw pre-change balances for up to 5 minutes. The job must force BOTH.
class Bot::UpdateMetricsJobTest < ActiveSupport::TestCase
  test 'forces base metrics AND the current-prices cache before broadcasting' do
    bot = create(:dca_index, user: create(:user))

    seq = sequence('refresh_then_broadcast')
    bot.expects(:metrics).with(force: true).returns({}).in_sequence(seq)
    bot.expects(:metrics_with_current_prices).with(force: true).returns({}).in_sequence(seq)
    Bot::BroadcastMetricsUpdateJob.expects(:perform_later).with(bot).in_sequence(seq)

    Bot::UpdateMetricsJob.perform_now(bot)
  end
end
