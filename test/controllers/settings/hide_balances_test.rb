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

  # The switch posts its own state rather than asking for a flip: two quick clicks can land out of
  # order, and a flip would leave the stored value disagreeing with the switch on screen.
  test 'the switch stores the state it posts' do
    patch settings_update_hide_balances_path, params: { hide_balances: '1' }
    assert @user.reload.hide_balances?

    patch settings_update_hide_balances_path, params: { hide_balances: '0' }
    assert_not @user.reload.hide_balances?
  end

  # An unchecked box posts nothing of its own — the hidden field beside it is what makes "off"
  # reach the server at all.
  test 'a post with no state at all reads as off' do
    @user.update!(hide_balances: true)

    patch settings_update_hide_balances_path

    assert_not @user.reload.hide_balances?
  end

  # A redirect, not a turbo-stream refresh: the preference changes a class on <body>, so the whole
  # document has to come back, and a refresh stream action is skipped while a visit is already in
  # flight — which is exactly what submitting this form is. The page would sit on the old state
  # until it was reloaded by hand.
  test 'the switch sends the user back to the page they were on, re-rendered' do
    patch settings_update_hide_balances_path,
          params: { hide_balances: '1' }, headers: { 'Referer' => "http://www.example.com#{tracker_path}" }

    assert_response :see_other
    assert_redirected_to tracker_path
  end

  # The menu is on every page, so "back where you were" is whatever the browser reports — which
  # makes this an open-redirect surface if it is taken at face value.
  test 'a referer pointing somewhere else entirely is refused' do
    patch settings_update_hide_balances_path,
          params: { hide_balances: '1' }, headers: { 'Referer' => 'http://evil.example/steal' }

    assert_redirected_to bots_path
    assert @user.reload.hide_balances?
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

  # A switch, so the row keeps one constant label and the control carries the state.
  test 'the menu row is a switch that reflects the stored state' do
    get bots_path
    assert_select 'form[action=?]', settings_update_hide_balances_path do
      assert_select '.toggle input[type=checkbox][name=hide_balances]'
      assert_select 'input[type=checkbox][checked]', false
      assert_select 'span', text: I18n.t('links.hide_balances')
    end

    @user.update!(hide_balances: true)

    get bots_path
    assert_select 'form[action=?] input[type=checkbox][checked]', settings_update_hide_balances_path
  end

  test 'both the desktop menu and the mobile one carry it' do
    get bots_path

    assert_select '.dropdown form[action=?] .toggle', settings_update_hide_balances_path
    assert_select '.menu-mobile form[action=?] .toggle', settings_update_hide_balances_path
  end

  # The whole line is the control. A menu row that only answers on its last two centimetres reads
  # as broken, so the row is a label and the click lands wherever it falls.
  test 'the whole row is the hit area, not just the switch' do
    get bots_path

    assert_select 'form[action=?] label', settings_update_hide_balances_path do
      assert_select 'input[type=checkbox][name=hide_balances]'
      assert_select 'span', text: I18n.t('links.hide_balances')
    end
  end

  # Flipping a switch is not choosing where to go: the menu has to survive the click, or a second
  # flip means reopening it.
  test 'the row opts out of the click that closes the menu' do
    get bots_path

    assert_select 'form[action=?] label[data-dropdown-keep-open]', settings_update_hide_balances_path
    # ...but the items that DO lead somewhere still close it.
    assert_select '.dropdown a.dropdown__item[data-dropdown-keep-open]', false
  end

  # The flip re-renders the page, and a menu that vanished mid-flip would be a menu you had to
  # reopen to change your mind.
  test 'the menus survive the re-render the switch causes' do
    get bots_path

    assert_select '#settings-menu[data-turbo-permanent]'
    assert_select '#hamburger[data-turbo-permanent]'
  end

  test 'signing out is still required to change anyone else\'s preference' do
    sign_out @user

    patch settings_update_hide_balances_path

    assert_redirected_to new_user_session_path
  end
end
