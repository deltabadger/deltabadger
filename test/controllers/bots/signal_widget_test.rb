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

  # The only place in the app that says a signal bot can be driven from the API, and with which id.
  test 'says the bot takes orders from the API, with its id' do
    get bot_path(id: @bot.id)

    assert_match I18n.t('bot.signal.api_hint', id: @bot.id), response.body
  end

  test 'a bot with no rules still renders, with the hint' do
    @signal.destroy!

    get bot_path(id: @bot.id)

    assert_response :ok
    assert_match I18n.t('bot.signal.api_hint', id: @bot.id), response.body
  end

  # fallback: false, or the English string satisfies every locale.
  test 'the hint exists natively in every locale' do
    I18n.available_locales.each do |locale|
      assert I18n.exists?('bot.signal.api_hint', locale, fallback: false), "#{locale}: bot.signal.api_hint is missing"
    end
  end
end
