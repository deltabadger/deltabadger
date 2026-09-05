require 'test_helper'

# The rule widget is where the user learns two things: the address to call, and whether anything has
# ever called it.
class Bots::SignalWidgetTest < ActionDispatch::IntegrationTest
  setup do
    create(:user, admin: true)
    @bot = create(:signal_bot, :started)
    @signal = create(:bot_signal, bot: @bot)
    sign_in @bot.user
  end

  test 'shows the full webhook URL' do
    get bot_path(id: @bot.id)

    assert_response :ok
    assert_match "http://www.example.com/hook/#{@signal.token}", response.body
  end

  test 'says the webhook has never been triggered' do
    get bot_path(id: @bot.id)

    assert_match I18n.t('bot.signal.never_triggered'), response.body
  end

  test 'says when the webhook was last triggered' do
    @signal.update!(last_triggered_at: 3.minutes.ago)

    get bot_path(id: @bot.id)

    assert_no_match I18n.t('bot.signal.never_triggered'), response.body
    assert_match(/3 minutes/, response.body)
  end
end
