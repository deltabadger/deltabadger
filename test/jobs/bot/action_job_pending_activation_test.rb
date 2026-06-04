require 'test_helper'

class Bot::ActionJobPendingActivationTest < ActiveSupport::TestCase
  include ActiveJob::TestHelper

  test 'reschedules without touching the exchange when the api_key is pending IBKR activation' do
    bot = create(:dca_single_asset, :started)
    bot.update!(status: :scheduled)
    bot.stubs(:next_action_job_at).returns(nil)
    bot.stubs(:api_key).returns(stub(pending_activation?: true))

    # The guard must fire BEFORE ensure_exchange_authenticated — a pending key never hits IBKR.
    bot.expects(:ensure_exchange_authenticated).never
    bot.expects(:execute_action).never
    Bot::ActionJob.stubs(:set).returns(stub(perform_later: true))
    Bot::BroadcastAfterScheduledActionJob.stubs(:perform_later)

    Bot::ActionJob.new.perform(bot)
  end
end
