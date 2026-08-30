require 'test_helper'

# The account page and the two "change a preference" actions behind it.
#
# These assert that the *call sites* resolve — a locale-presence test would pass
# happily while a view asks for a key one level too deep, which is exactly how
# `links.logout` and `settings.language_and_timezone.timezone.*` survived. The view
# helper answers a missing key with a `translation_missing` span whose text is the
# humanised last segment, so the page looks almost right in English and shows English
# in the other fourteen locales.
class SettingsControllerTest < ActionDispatch::IntegrationTest
  include Devise::Test::IntegrationHelpers

  setup do
    create(:user, admin: true, setup_completed: true) # platform requires an admin to exist
    @user = create(:user, time_zone: 'UTC', locale: 'en')
    sign_in @user
  end

  test 'account page resolves every translation it references' do
    get settings_account_path

    assert_response :success
    assert_no_match(/translation_missing/, @response.body)
  end

  test 'navbar renders the logout link' do
    # In English the missing-translation span humanises the last key segment to
    # "Logout" and looks entirely correct, so this only bites in a locale where the
    # real translation differs.
    @user.update!(locale: 'pl')
    get settings_account_path(locale: 'pl')

    assert_includes @response.body, I18n.t('links.log_out', locale: :pl)
  end

  test 'the clock caption under the time-zone picker is translated' do
    get settings_account_path

    assert_includes @response.body, I18n.t('settings.language_and_timezone.current_time_is')
  end

  test 'a time-zone change flashes a translated confirmation' do
    patch settings_update_time_zone_path, params: { user: { time_zone: 'Warsaw' } }

    assert_response :success
    assert_equal I18n.t('settings.language_and_timezone.updated'), flash[:notice]
  end

  # Both selects are inside turbo frames, so an out-of-range value takes a crafted
  # request — but the model validates inclusion, so it is reachable, and today it
  # falls through to a template that does not exist.
  test 'a rejected time zone reports the error instead of raising' do
    patch settings_update_time_zone_path, params: { user: { time_zone: 'Mars/Olympus_Mons' } }

    assert_response :unprocessable_entity
    assert_equal 'UTC', @user.reload.time_zone
    assert_includes @response.body, 'salert--danger'
  end

  test 'a rejected locale reports the error instead of raising' do
    patch settings_update_locale_path, params: { user: { locale: 'xx' } }

    assert_response :unprocessable_entity
    assert_equal 'en', @user.reload.locale
    assert_includes @response.body, 'salert--danger'
  end

  # The two values that slipped past the validators entirely rather than failing them:
  # `allow_blank` waved "" through, the row was written, and only then did I18n refuse the
  # locale — a 500 with a corrupted row behind it, from a plain form post. `allow_nil` on
  # time_zone waved nil into a `null: false` column.
  test 'a blank locale clears the preference instead of being persisted as one' do
    patch settings_update_locale_path, params: { user: { locale: '' } }

    assert_response :success
    assert_nil @user.reload.locale
  end

  # Absent and wrong are different answers. The schema declares `null: false default "UTC"`,
  # so an absent zone means the default — on an update as much as on an insert, which is the
  # half that used to reach the NOT NULL constraint instead.
  test 'a null time zone falls back to the default instead of reaching the NOT NULL constraint' do
    @user.update!(time_zone: 'Warsaw')

    patch settings_update_time_zone_path,
          params: { user: { time_zone: nil } }.to_json,
          headers: { 'Content-Type' => 'application/json' }

    assert_response :success
    assert_equal 'UTC', @user.reload.time_zone
  end

  # Defect 4 in the report: the name/email/password failure branches set no flash and
  # re-render the field with `value: ""`. What saves them is the inline_form_errors
  # initializer — the reason is attached to the field, inside the frame Turbo extracts.
  test 'a rejected name comes back with the reason attached to the field' do
    patch settings_update_name_path,
          params: { user: { name: 'N4me' } },
          headers: { 'Turbo-Frame' => 'update_name_form' }

    assert_response :unprocessable_entity
    assert_includes @response.body, I18n.t('devise.registrations.new.name_invalid')
  end

  test 'a wrong current password comes back with the reason attached to the field' do
    patch settings_update_password_path,
          params: { user: { current_password: 'WrongPassword1!', password: 'NewPassword1!',
                            password_confirmation: 'NewPassword1!' } },
          headers: { 'Turbo-Frame' => 'update_password_form' }

    assert_response :unprocessable_entity
    assert_includes @response.body, 'form__info--invalid'
  end

  # The picker lives in the menu, which is drawn on every signed-in page and drawn twice — the
  # desktop dropdown and the mobile drawer — so the count is per menu.
  test 'the settings menu offers every supported fiat denominator, by its symbol' do
    get settings_account_path

    assert_select '.segmented[data-currency] .segmented__option',
                  count: User::DISPLAY_CURRENCIES.size * 2
    assert_select '.segmented[data-currency] .segmented__option', text: 'Fr.'
    assert_select 'select[name="user[display_currency]"]', count: 0
  end

  # redirect_back, not a refresh stream: the picker is not in a turbo-frame any more, and a
  # refresh action is skipped while the submit's own visit is in flight.
  test 'a currency change flashes a translated confirmation and comes back to the page' do
    patch settings_update_display_currency_path,
          params: { user: { display_currency: 'PLN' } },
          headers: { 'HTTP_REFERER' => 'http://www.example.com/tracker' }

    assert_redirected_to '/tracker'
    assert_response :see_other
    assert_equal 'PLN', @user.reload.display_currency
    assert_equal I18n.t('settings.language_and_timezone.currency_updated'), flash[:notice]
  end

  # Same crafted-request reach as the other two selects, and the same column shape as
  # time_zone: `null: false default "USD"`, so both "wrong" and "absent" need an answer.
  test 'a rejected currency reports the error instead of raising' do
    patch settings_update_display_currency_path, params: { user: { display_currency: 'XYZ' } }

    assert_response :unprocessable_entity
    assert_equal 'USD', @user.reload.display_currency
    assert_includes @response.body, 'salert--danger'
  end

  test 'a blank currency falls back to the default instead of reaching the NOT NULL constraint' do
    @user.update!(display_currency: 'PLN')

    patch settings_update_display_currency_path, params: { user: { display_currency: '' } }

    assert_response :see_other
    assert_equal 'USD', @user.reload.display_currency
  end
end
