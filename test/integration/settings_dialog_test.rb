require 'test_helper'

# Settings is one dialog — a sidebar of five sections and the pane they open — opened from the
# menu over whatever page is behind it.
class SettingsDialogTest < ActionDispatch::IntegrationTest
  setup do
    @admin = create(:user, admin: true, setup_completed: true) # also the onboarding gate
    @user = create(:user, setup_completed: true)
    sign_in @user
  end

  test 'every section is the dialog, with the sidebar marking where it is' do
    sign_in @admin

    { settings_account_path => 'account', settings_api_keys_path => 'api_keys', settings_connect_path => 'connect',
      settings_mcp_path => 'mcp', settings_api_path => 'api' }.each do |path, section|
      get path

      assert_response :success
      assert_select 'turbo-frame#modal dialog.sdialog turbo-frame#settings_pane' do
        assert_select '.settings__item', 5
        assert_select '.settings__item--active[aria-current=page][href=?]', path, 1, "#{section} is not the active item"
      end
    end
  end

  # Connect holds container-wide sources (admin only) and IBKR. Without either it would open empty.
  test 'Connect is left out of the sidebar when it would be empty' do
    StockTradingSettings.stubs(:ibkr_available?).returns(false)

    get settings_account_path

    assert_select '.settings__item', 4
    assert_select '.settings__item[href=?]', settings_connect_path, 0
  end

  # Visited as a page nothing is behind the dialog, so closing it has to go somewhere. Opened from
  # the menu it closes onto the page it was opened over.
  test 'a page visit closes to the bots, a frame visit closes in place' do
    get settings_account_path
    assert_select 'dialog[data-modal--base-close-url-value=?]', bots_path

    get settings_account_path, headers: { 'Turbo-Frame' => 'modal' }
    assert_select 'dialog.sdialog'
    assert_select 'dialog[data-modal--base-close-url-value]', 0
  end

  test 'the menu has one Settings entry, opening the dialog in the modal frame' do
    get tracker_path

    assert_select '#settings-menu a[href=?][data-turbo-frame=modal]', settings_account_path, text: I18n.t('links.settings')
    assert_select '.menu-mobile a[href=?][data-turbo-frame=modal]', settings_account_path
    assert_select "a[href='#{settings_connect_path}'], a[href='#{settings_api_keys_path}'], a[href='#{settings_api_path}']", 0
  end

  # A page refresh would re-render the page behind, where the dialog's frame is empty — closing
  # it under the user. A saved row answers with itself.
  test 'saving a row answers with the row and a flash, never a page refresh' do
    patch settings_update_time_zone_path, params: { user: { time_zone: 'Warsaw' } }, as: :turbo_stream

    assert_response :success
    assert_select 'turbo-stream[action=replace][target=update_time_zone_form]', 1
    assert_select 'turbo-stream[action=prepend][target=flash]', 1
    assert_select 'turbo-stream[action=refresh]', 0
  end

  # The whole row flips its switch: a second label for the same checkbox, laid over the row.
  test 'every switch row carries a label for its own checkbox' do
    get settings_mcp_path

    assert_select '.srow .sswitch' do |switches|
      switches.each do |switch|
        id = switch.at('input[type=checkbox]')['id']
        assert id.present?, 'a switch checkbox without an id cannot be labelled'
        assert_equal 1, switch.css("label.sswitch__hit[for='#{id}']").size
      end
    end
  end

  test 'two-factor opens inside its row' do
    get settings_account_path
    assert_select 'turbo-frame#two_fa a[href=?]', settings_edit_two_fa_path

    get settings_edit_two_fa_path, headers: { 'Turbo-Frame' => 'two_fa' }

    assert_response :success
    assert_select 'turbo-frame#two_fa form[action=?] input[name=?]', settings_update_two_fa_path, 'user[otp_code_token]'
    assert_select 'turbo-frame#two_fa .qrcode'
  end

  test 'a wrong code keeps the row open with the error' do
    patch settings_update_two_fa_path, params: { user: { otp_code_token: '000000' } }

    assert_response :unprocessable_entity
    assert_select 'turbo-frame#two_fa .form__info--invalid'
  end

  test 'removing a key asks first, in place, and answers with the list' do
    api_key = create(:api_key, user: @user)

    get settings_api_keys_path
    assert_select 'turbo-frame#api_keys details.sconfirm form[action=?]', settings_destroy_api_key_path(id: api_key.id)
    assert_select 'turbo-frame#api_keys', /#{api_key.exchange.name}/
    assert_select 'turbo-frame#api_keys .srow__icon svg', 1, 'each key row leads with its exchange logo'

    delete settings_destroy_api_key_path(id: api_key.id)

    assert_response :success
    assert_select 'turbo-stream[action=replace][target=api_keys]', 1
    assert_not ApiKey.exists?(api_key.id)
  end

  test 'the key permissions open in the pane, with a way back' do
    api_key = create(:api_key, user: @user)

    get settings_api_key_permissions_path(id: api_key.id), headers: { 'Turbo-Frame' => 'settings_pane' }

    assert_response :success
    assert_select 'turbo-frame#settings_pane a.settings__back[href=?]', settings_api_keys_path
    assert_select '.settings__item--active[href=?]', settings_api_keys_path
  end
end

class SettingsDialogOutlineTest < ActionDispatch::IntegrationTest
  # The dialog's controls are all the outline kind — an unbordered one reads as a different family.
  test 'every button and input in the dialog is outlined' do
    sign_in create(:user, admin: true, setup_completed: true)

    %i[settings_account_path settings_api_keys_path settings_connect_path settings_mcp_path settings_api_path].each do |path|
      get send(path)
      assert_select '.settings .rbutton:not(.rbutton--outline)', 0, "#{path} has an unoutlined button"
      assert_select '.settings .rinput:not(.rinput--outline)', 0, "#{path} has an unoutlined input"
    end
  end
end

# A language change re-renders everything, so it is a full page load — but to the page the dialog was
# opened over, in the new language, with the dialog open on it again; never to a bare settings page.
class SettingsLocaleChangeTest < ActionDispatch::IntegrationTest
  setup do
    create(:user, admin: true, setup_completed: true) # onboarding gate
    sign_in create(:user, setup_completed: true)
  end

  test 'changing the language comes back to the page behind, dialog reopened' do
    patch settings_update_locale_path, params: { user: { locale: 'pl' } },
                                       headers: { 'Referer' => 'http://www.example.com/bots' }, as: :turbo_stream

    assert_select 'turbo-stream[action=reload][target=?]', '/pl/bots?settings=account'
  end

  test 'from a settings page there is no page behind, so it stays on settings' do
    patch settings_update_locale_path, params: { user: { locale: 'pl' } },
                                       headers: { 'Referer' => 'http://www.example.com/settings/account' }, as: :turbo_stream

    assert_select 'turbo-stream[action=reload][target=?]', '/pl/settings/account'
  end

  test 'only the referer path is used, swapping the locale prefix' do
    patch settings_update_locale_path(locale: 'de'), params: { user: { locale: 'en' } },
                                                     headers: { 'Referer' => 'https://evil.example/de/tracker?x=1' }, as: :turbo_stream

    assert_select 'turbo-stream[action=reload][target=?]', '/tracker?settings=account'
  end

  test 'a protocol-relative or missing referer lands on settings, never off-site' do
    patch settings_update_locale_path, params: { user: { locale: 'pl' } },
                                       headers: { 'Referer' => 'http://www.example.com//evil.example/x' }, as: :turbo_stream
    assert_select 'turbo-stream[action=reload][target=?]', '/pl/settings/account'

    patch settings_update_locale_path, params: { user: { locale: 'pl' } }, as: :turbo_stream
    assert_select 'turbo-stream[action=reload][target=?]', '/pl/settings/account'
  end

  test '?settings opens the dialog over the page, and only for a real section' do
    get bots_path(settings: 'account')
    assert_select 'turbo-frame#modal[src=?]', settings_account_path

    get bots_path(settings: 'nope')
    assert_select 'turbo-frame#modal[src]', 0
  end
end
