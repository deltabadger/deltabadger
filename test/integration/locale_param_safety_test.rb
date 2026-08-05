require 'test_helper'

class LocaleParamSafetyTest < ActionDispatch::IntegrationTest
  setup { create(:user, admin: true, setup_completed: true) }

  # NOTE: the path must NOT carry a locale segment. `/en/login` sets locale=en as a path
  # parameter (config/routes.rb:68) and Rails merges path parameters over query
  # parameters, so `/en/login?locale=zz` reaches the controller as `en` and would pass
  # before the fix. Only the unprefixed path lets the query param through.
  test 'an unknown locale falls back rather than raising' do
    get '/login', params: { locale: 'zz' }
    assert_response :success
  end

  test 'a junk locale falls back rather than raising' do
    get '/login', params: { locale: '../../etc/passwd' }
    assert_response :success
  end

  test 'the dashboard redirect cannot be pointed off-site' do
    get '/dashboard', params: { locale: '/evil.com' }

    assert_response :redirect
    refute_match %r{\A(https?:)?//evil\.com}, response.headers['Location'].to_s
    assert_match %r{\Ahttp://www\.example\.com/}, response.headers['Location'].to_s
  end

  test 'the settings redirect cannot be pointed off-site' do
    get '/settings', params: { locale: '/evil.com' }

    assert_response :redirect
    refute_match %r{\A(https?:)?//evil\.com}, response.headers['Location'].to_s
    assert_match %r{\Ahttp://www\.example\.com/}, response.headers['Location'].to_s
  end

  # Guards against a validator that always fails closed to the default locale (e.g. comparing
  # a String param against the Symbol keys `I18n.available_locales` returns) — that would still
  # pass every test above while silently forcing English for every visitor.
  test 'a legitimate locale is actually honoured, not just accepted' do
    get '/login', params: { locale: 'pl' }

    assert_response :success
    assert_includes response.body, 'Witaj ponownie!'
  end

  # settings#update_locale reads current_user.locale straight into I18n.t after saving it,
  # bypassing switch_locale's own guard entirely. The fix belongs on the column, not this
  # call site, so any other future reader of current_user.locale is covered too.
  test 'an invalid locale submitted through the account form is rejected, not persisted' do
    user = create(:user, admin: true, setup_completed: true, locale: 'en')
    sign_in user

    patch settings_update_locale_path, params: { user: { locale: 'zz' } }

    assert_response :success
    assert_equal 'en', user.reload.locale
  end
end
