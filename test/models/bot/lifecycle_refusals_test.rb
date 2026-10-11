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
  test 'Start reads the current generation past the query cache' do
    bot = create(:dca_single_asset, :stopped)
    ActiveRecord::Base.cache do
      bot.class.find_by(id: bot.id)
      bot.class.connection.raw_connection.execute(
        "UPDATE bots SET transient_data = json_set(transient_data, '$.run_generation', 1) WHERE id = #{bot.id}"
      )
      assert bot.start(start_fresh: false)
    end

    assert_equal 2, bot.reload.transient_data['run_generation']
  end

  test 'a stale STOP cannot restore an earlier run generation from transient data' do
    bot = create(:dca_single_asset, :started)
    stale = Bot.find(bot.id)
    stale.missed_quote_amount = 123
    assert Bot.find(bot.id).stop
    assert Bot.find(bot.id).start(start_fresh: false)
    generation = Bot.find(bot.id).transient_data['run_generation']

    assert stale.stop

    assert_equal generation, Bot.find(bot.id).transient_data['run_generation']
  end

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

  # A setting that no longer validates (here a rebalance threshold of 0 with rebalancing off) has
  # nothing to do with stopping. Refusing the stop left the bot trading.
  test 'a stop goes through when unrelated settings no longer validate' do
    bot = invalid_working_bot
    bot.expects(:cancel_scheduled_action_jobs)

    assert bot.stop
    assert_predicate bot.reload, :stopped?
    assert_not_nil bot.stopped_at
    assert_equal 'stopped', bot.bot_activity_logs.last&.event
  end

  test 'the REST stop of such a bot succeeds' do
    bot = invalid_working_bot

    result = BotApi::Bots::Stop.call(user: bot.user, bot_id: bot.id)

    assert_predicate result, :success?
    assert_predicate bot.reload, :stopped?
  end

  test 'STOP cannot restore a carry captured before another writer committed' do
    bot = create(:dca_single_asset, :started)
    bot.set_missed_quote_amount
    current = Bot.find(bot.id)
    current.update_column(:transient_data, current.transient_data.merge('missed_quote_amount' => 37))

    assert bot.stop

    assert_equal 37.to_d, bot.reload.missed_quote_amount
  end

  # Archive still validates, and refuses before it stops anything: a refused archive changes nothing.
  test 'an archive of such a bot is refused whole' do
    bot = invalid_working_bot
    before = bot.reload.attributes.deep_dup

    refute bot.archive
    assert_equal before, Bot.find(bot.id).attributes
  end

  private

  def invalid_working_bot
    bot = create(:dca_multi_asset, :started)
    bot.update_column(:settings, bot.settings.merge('rebalance_enabled' => false, 'rebalance_threshold' => 0))
    bot = Bot.find(bot.id)
    bot.set_missed_quote_amount
    assert_not bot.valid?, 'precondition: the stored settings do not validate'
    bot
  end
end
