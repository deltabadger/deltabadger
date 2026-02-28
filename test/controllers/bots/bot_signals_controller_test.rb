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
    patch bot_bot_signal_path(bot_id: @bot.id, id: @signal.id), params: {
      bot_signal: { direction: :sell }
    }, as: :turbo_stream
    assert_response :ok
    assert_predicate @signal.reload, :sell?
  end

  test 'update changes signal amount' do
    patch bot_bot_signal_path(bot_id: @bot.id, id: @signal.id), params: {
      bot_signal: { amount: 250 }
    }, as: :turbo_stream
    assert_response :ok
    assert_equal 250, @signal.reload.amount
  end

  test 'destroy removes signal when not the last one' do
    second_signal = create(:bot_signal, bot: @bot)
    assert_difference 'BotSignal.count', -1 do
      delete bot_bot_signal_path(bot_id: @bot.id, id: second_signal.id), as: :turbo_stream
    end
    assert_response :ok
  end

  test 'destroy prevents removing the last signal' do
    assert_no_difference 'BotSignal.count' do
      delete bot_bot_signal_path(bot_id: @bot.id, id: @signal.id), as: :turbo_stream
    end
    assert_response :unprocessable_entity
  end

  test 'requires authentication' do
    sign_out @bot.user

    post bot_bot_signals_path(bot_id: @bot.id), as: :turbo_stream
    assert_redirected_to new_user_session_path
  end
end
