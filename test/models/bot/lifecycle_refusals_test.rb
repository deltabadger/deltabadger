require 'test_helper'

# Start and stop each refuse, or go through, in the one place every caller shares: the web
# controllers, the REST/MCP services and the jobs all call Bot::Lifecycle.
class Bot::LifecycleRefusalsTest < ActiveSupport::TestCase
  setup do
    Bot::ActionJob.stubs(:perform_later)
    Bot::ActionJob.stubs(:set).returns(stub(perform_later: true))
    Bot::BroadcastAfterScheduledActionJob.stubs(:perform_later)
  end

  # The REST API answers this with 409 before it reaches the model; the web did not ask at all.
  test 'a start on a working bot is refused and writes nothing' do
    bot = create(:dca_single_asset, started_at: 2.days.ago)
    %i[scheduled executing retrying waiting].each do |status|
      bot.update_column(:status, Bot.statuses[status])
      bot = Bot.find(bot.id)
      before = bot.reload.attributes.deep_dup
      bot.expects(:cancel_scheduled_action_jobs).never

      refute bot.start(start_fresh: true), status
      assert_equal [I18n.t('engine.already_running')], bot.errors[:base], status
      assert_equal before, bot.reload.attributes, status
    end
  end

  # The in-memory copy can be stale: a page loaded while the bot was stopped, started since.
  test 'a start is refused when the bot was started after this copy was loaded' do
    bot = create(:dca_single_asset, status: :stopped)
    Bot.find(bot.id).update_column(:status, Bot.statuses[:scheduled])

    refute bot.start(start_fresh: true)
    assert_equal [I18n.t('engine.already_running')], bot.errors[:base]
  end
end
