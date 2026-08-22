# frozen_string_literal: true

require 'test_helper'

# A redirect a Turbo-managed form submission lands on carries over the original request's
# turbo-stream Accept preference — browsers replay the same headers when following a redirect —
# so the bot page can be asked for as turbo_stream by a plain revisit that has nothing to do with
# the orders-pagination frame. That frame is the only legitimate turbo_stream caller and always
# sends `decimals`; anything else must fall back to a normal page render instead of blowing up.
class Bots::ShowTurboStreamRevisitTest < ActionDispatch::IntegrationTest
  setup do
    create(:user, admin: true, setup_completed: true) # satisfies the onboarding gate
    @bot = create(:dca_single_asset, :started)
    sign_in @bot.user
  end

  test 'a turbo_stream revisit with no decimals renders the full page instead of erroring' do
    get bot_path(id: @bot.id, format: :turbo_stream)

    assert_response :ok
    assert_match @bot.label, @response.body
  end
end
