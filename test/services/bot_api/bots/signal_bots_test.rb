# frozen_string_literal: true

require 'test_helper'

# The bot services were written against the DCA types and reach for the buy-side carry before
# every lifecycle write. A signal bot has no carry; listing it and then refusing to start it with
# a 500 is worse than either exposing or hiding it.
class BotApi::Bots::SignalBotsTest < ActiveSupport::TestCase
  setup do
    @user = create(:user, admin: true)
    @bot = create(:signal_bot, user: @user, status: :stopped)
    create(:bot_signal, bot: @bot)
  end

  test 'Start starts a signal bot' do
    result = BotApi::Bots::Start.call(user: @user, bot_id: @bot.id)

    assert result.success?, result.error_message
    assert_predicate @bot.reload, :scheduled?
  end

  test 'Stop stops a signal bot' do
    @bot.update!(status: :scheduled)

    result = BotApi::Bots::Stop.call(user: @user, bot_id: @bot.id)

    assert result.success?, result.error_message
    assert_predicate @bot.reload, :stopped?
  end

  test 'UpdateSettings renames a signal bot' do
    result = BotApi::Bots::UpdateSettings.call(user: @user, bot_id: @bot.id, label: 'Breakout')

    assert result.success?, result.error_message
    assert_equal 'Breakout', @bot.reload.label
  end

  # A signal bot has no per-order amount — the sizing lives on its rules.
  test 'UpdateSettings refuses a quote amount for a signal bot' do
    result = BotApi::Bots::UpdateSettings.call(user: @user, bot_id: @bot.id, quote_amount: 50)

    assert_not result.success?
    assert_equal :validation_failed, result.status
  end
end
