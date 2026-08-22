# frozen_string_literal: true

require 'test_helper'

# The "Hide balances" toggle in the bottom-left menu. It is a preference on the user, not a
# per-page state, so it survives a reload and reaches every screen at once.
class Settings::HideBalancesTest < ActionDispatch::IntegrationTest
  include Turbo::Broadcastable::TestHelper

  setup do
    create(:user, admin: true) # satisfies the onboarding gate (an admin must exist)
    @user = create(:user)
    sign_in @user
  end

  test 'a user is not hiding balances to begin with' do
    assert_not @user.hide_balances?
  end

  test 'the toggle turns hiding on and off again' do
    patch settings_update_hide_balances_path
    assert @user.reload.hide_balances?

    patch settings_update_hide_balances_path
    assert_not @user.reload.hide_balances?
  end

  # The preference changes every figure on the page, not one frame, so the answer is a refresh —
  # the same answer the display-currency select gives.
  test 'the toggle answers with a page refresh' do
    patch settings_update_hide_balances_path

    assert_response :success
    assert_match 'turbo-stream action="refresh"', @response.body
  end

  # ...and the user's OTHER open tabs have to hear about it too, or a second window keeps
  # rendering the markup it was served before the toggle. Account-wide, so the layout subscribes
  # every signed-in page rather than the three that happen to show balances.
  test 'the toggle refreshes the account\'s other open tabs' do
    assert_turbo_stream_broadcasts(["user_#{@user.id}", :preferences], count: 1) do
      patch settings_update_hide_balances_path
    end
  end

  # ...but only the pages that state balances listen. A refresh REPLACES the document, so a
  # settings page listening for one would throw away a half-typed API key or password.
  test 'a page that states no balances does not listen for the refresh' do
    get settings_account_path

    assert_select 'turbo-cable-stream-source', false
  end

  test 'the pages that state balances do listen' do
    get tracker_path

    assert_select 'turbo-cable-stream-source', 3
  end

  test 'the body says whether balances are hidden' do
    get bots_path
    assert_select 'body:not(.hide-balances)'

    @user.update!(hide_balances: true)

    get bots_path
    assert_select 'body.hide-balances'
  end

  # The item says what the click will do, so there is no check state to read.
  test 'the menu item offers the opposite of the current state' do
    get bots_path
    assert_select 'form[action=?] button', settings_update_hide_balances_path,
                  text: I18n.t('links.hide_balances')

    @user.update!(hide_balances: true)

    get bots_path
    assert_select 'form[action=?] button', settings_update_hide_balances_path,
                  text: I18n.t('links.show_balances')
  end

  test 'signing out is still required to change anyone else\'s preference' do
    sign_out @user

    patch settings_update_hide_balances_path

    assert_redirected_to new_user_session_path
  end
end
