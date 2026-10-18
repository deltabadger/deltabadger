require 'test_helper'

class Bots::BotSignalsControllerTest < ActionDispatch::IntegrationTest
  setup do
    @admin = create(:user, admin: true)
    @bot = create(:signal_bot, :started)
    @signal = create(:bot_signal, bot: @bot)
    sign_in @bot.user
  end

  test 'create adds a new signal' do
    @bot.update!(status: :stopped)
    assert_difference 'BotSignal.count', 1 do
      post bot_bot_signals_path(bot_id: @bot.id), as: :turbo_stream
    end
    assert_response :ok

    new_signal = @bot.bot_signals.last
    assert_predicate new_signal, :buy?
    assert_equal 100, new_signal.amount
  end

  test 'update changes signal direction' do
    @bot.update!(status: :stopped)
    patch bot_bot_signal_path(bot_id: @bot.id, id: @signal.id), params: {
      bot_signal: { direction: :sell }
    }, as: :turbo_stream
    assert_response :ok
    assert_predicate @signal.reload, :sell?
  end

  test 'update changes signal amount' do
    @bot.update!(status: :stopped)
    patch bot_bot_signal_path(bot_id: @bot.id, id: @signal.id), params: {
      bot_signal: { amount: 250 }
    }, as: :turbo_stream
    assert_response :ok
    assert_equal 250, @signal.reload.amount
  end

  test 'destroy removes signal when not the last one' do
    @bot.update!(status: :stopped)
    second_signal = create(:bot_signal, bot: @bot)
    assert_difference 'BotSignal.count', -1 do
      delete bot_bot_signal_path(bot_id: @bot.id, id: second_signal.id), as: :turbo_stream
    end
    assert_response :ok
  end

  test 'destroy prevents removing the last signal' do
    @bot.update!(status: :stopped)
    assert_no_difference 'BotSignal.count' do
      delete bot_bot_signal_path(bot_id: @bot.id, id: @signal.id), as: :turbo_stream
    end
    assert_response :unprocessable_entity
  end

  # The controls are disabled while the bot runs; a stale tab still posts. A rule edited under a
  # running bot changes what the next webhook call trades, so the server refuses it too.
  test 'a running bot refuses a new rule, and the settings come back as they are' do
    assert_no_difference 'BotSignal.count' do
      post bot_bot_signals_path(bot_id: @bot.id), as: :turbo_stream
    end

    assert_running_refusal
  end

  test 'a running bot refuses a rule change' do
    patch bot_bot_signal_path(bot_id: @bot.id, id: @signal.id), params: {
      bot_signal: { direction: :sell, amount: 250, amount_type: :percentage }
    }, as: :turbo_stream

    assert_running_refusal
    @signal.reload
    assert_predicate @signal, :buy?
    assert_equal 100, @signal.amount
  end

  test 'a running bot refuses switching a rule off' do
    patch bot_bot_signal_path(bot_id: @bot.id, id: @signal.id), params: { bot_signal: { enabled: '0' } }, as: :turbo_stream

    assert_running_refusal
    assert_predicate @signal.reload, :enabled?
  end

  test 'a running bot refuses removing a rule' do
    second = create(:bot_signal, bot: @bot)

    assert_no_difference 'BotSignal.count' do
      delete bot_bot_signal_path(bot_id: @bot.id, id: second.id), as: :turbo_stream
    end

    assert_running_refusal
  end

  test 'the refusal exists natively in every locale' do
    I18n.available_locales.each do |locale|
      assert I18n.exists?('errors.bots.signal_locked_while_running', locale, fallback: false), "missing in #{locale}"
    end
  end

  test 'requires authentication' do
    sign_out @bot.user

    post bot_bot_signals_path(bot_id: @bot.id), as: :turbo_stream
    assert_redirected_to new_user_session_path
  end

  private

  def assert_running_refusal
    assert_response :unprocessable_entity
    assert_match I18n.t('errors.bots.signal_locked_while_running'), response.body
    assert_select 'turbo-stream[action=replace][target=settings]'
  end
end
