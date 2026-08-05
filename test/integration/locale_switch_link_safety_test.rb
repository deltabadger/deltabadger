require 'test_helper'

# The language dropdown renders on unauthenticated pages, so a crafted link is
# deliverable to anyone. Guards the whole rendered response, not just the helper.
class LocaleSwitchLinkSafetyTest < ActionDispatch::IntegrationTest
  setup { create(:user, admin: true, setup_completed: true) }

  test 'login page language links ignore an attacker-supplied host' do
    get '/en/login', params: { host: 'evil.com' }

    assert_response :success
    refute_includes response.body, 'evil.com'
  end

  test 'login page language links ignore an attacker-supplied protocol' do
    get '/en/login', params: { protocol: 'javascript', host: '%0Aalert(document.domain)%0A%2F%2F' }

    assert_response :success
    refute_match(/href="javascript:/, response.body)
  end

  test 'setup page language links ignore an attacker-supplied host' do
    User.delete_all # /setup is only reachable while no admin exists
    # /setup is defined outside the `(:locale)` route scope (config/routes.rb) — it has
    # no locale-prefixed path, unlike /login.
    get '/setup', params: { host: 'evil.com' }

    assert_response :success
    refute_includes response.body, 'evil.com'
  end
end
