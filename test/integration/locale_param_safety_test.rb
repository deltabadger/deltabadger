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
  end

  test 'the settings redirect cannot be pointed off-site' do
    get '/settings', params: { locale: '/evil.com' }

    assert_response :redirect
    refute_match %r{\A(https?:)?//evil\.com}, response.headers['Location'].to_s
  end
end
