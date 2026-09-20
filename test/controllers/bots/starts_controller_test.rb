require 'test_helper'

# The restart question is an interval bot's question: carry the buy the stop went through, or skip
# to the next checkpoint. A bot with no schedule has neither, so the modal must not try to ask it —
# it used to 500 on the first thing it asked the bot (Bot::Lifecycle#restarting_within_interval?,
# which a schedule-less bot does not have).
class Bots::StartsControllerTest < ActionDispatch::IntegrationTest
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
end
