require 'test_helper'

# The restart question is an interval bot's question: carry the buy the stop went through, or skip
# to the next checkpoint. A bot with no schedule has neither, so the modal must not try to ask it —
# it used to 500 on the first thing it asked the bot (Bot::Lifecycle#restarting_within_interval?,
# which a schedule-less bot does not have).
class Bots::StartsControllerTest < ActionDispatch::IntegrationTest
  TURBO_STREAM_ACCEPT = 'text/vnd.turbo-stream.html, text/html'.freeze

  setup do
    create(:user, admin: true, setup_completed: true) # onboarding gate
    @user = create(:user)
    @user.update!(wash_sale_enabled: false) # not the question under test
    sign_in @user
  end

  test 'a bot with no interval is not asked the restart question' do
    bot = create(:signal_bot, user: @user, status: :stopped)

    get edit_bot_start_path(bot_id: bot.id)

    assert_response :success
    assert_select 'turbo-frame#modal'
    assert_select 'form[action^=?]', bot_start_path(bot_id: bot.id), count: 0
  end

  # A tab left open from before the bot was started still shows Start. Restarting a working bot
  # re-anchors its schedule and runs a tick at once, which can place an extra order.
  test 'a start on a bot that is already working is refused and changes nothing' do
    bot = create(:dca_single_asset, :started, user: @user, started_at: 2.days.ago)
    bot.update_column(:transient_data, bot.transient_data.merge('last_action_job_at' => 1.hour.ago.iso8601))
    before = bot.reload.attributes
    Bot::ActionJob.expects(:perform_later).never
    Bot::ActionJob.expects(:set).never

    patch bot_start_path(bot_id: bot.id, start_fresh: true), headers: { 'Accept' => TURBO_STREAM_ACCEPT }

    assert_response :unprocessable_entity
    assert_match I18n.t('engine.already_running'), response.body
    assert_equal before, bot.reload.attributes
  end

  test 'a start without start_fresh starts fresh instead of failing' do
    bot = create(:dca_single_asset, user: @user, status: :stopped)
    stub_enqueues

    patch bot_start_path(bot_id: bot.id), headers: { 'Accept' => TURBO_STREAM_ACCEPT }

    assert_response :success
    assert_predicate bot.reload, :scheduled?
  end

  test 'an unreadable start_fresh is refused, not a server error' do
    bot = create(:dca_single_asset, user: @user, status: :stopped)
    stub_enqueues

    patch bot_start_path(bot_id: bot.id, start_fresh: 'junk'), headers: { 'Accept' => TURBO_STREAM_ACCEPT }

    assert_response :unprocessable_entity
    assert_match I18n.t('engine.invalid_start_fresh'), response.body
    assert_predicate bot.reload, :stopped?
  end

  test 'STOP shows the in-flight warning in its browser response' do
    bot = create(:dca_single_asset, :started, user: @user)
    bot.update_columns(placement_intents: { 'sent' => { 'run_identity' => bot.run_identity } })

    patch bot_stop_path(bot_id: bot.id), headers: { 'Accept' => TURBO_STREAM_ACCEPT }

    assert_response :success
    assert_predicate bot.reload, :stopped?
    assert_includes response.body, Automation::Statusable::STOP_DISPATCH_NOTICE
  end

  private

  def stub_enqueues
    Bot::ActionJob.stubs(:perform_later)
    Bot::ActionJob.stubs(:set).returns(stub(perform_later: true))
    Bot::BroadcastAfterScheduledActionJob.stubs(:perform_later)
    Bots::DcaSingleAsset.any_instance.stubs(:check_missed_quote_amount_was_set).returns(true)
  end
end
