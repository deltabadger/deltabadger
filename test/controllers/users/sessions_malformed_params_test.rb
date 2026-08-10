# frozen_string_literal: true

require 'test_helper'

# Sign-in reads params[:user][:password] straight out of the request before anything has
# established that either level is there, so a request that simply omits one — or sends
# user as a scalar — raised out of the controller. That is an unauthenticated 500 on a
# route anyone can reach, and 500s are worth denying on principle here: they are noise in
# the logs that real faults have to be found in, and a cheap way to make the single-threaded
# server spend a thread.
class Users::SessionsMalformedParamsTest < ActionDispatch::IntegrationTest
  setup { create(:user, admin: true, setup_completed: true) }

  test 'a sign-in with no user param does not raise' do
    post '/login'

    assert_no_server_error
  end

  test 'a sign-in with no password does not raise' do
    post '/login', params: { user: { email: 'a@b.test' } }

    assert_no_server_error
  end

  test 'a sign-in with no email does not raise' do
    post '/login', params: { user: { password: 'whatever' } }

    assert_no_server_error
  end

  test 'a sign-in with user sent as a scalar does not raise' do
    post '/login', params: { user: 'not-a-hash' }

    assert_no_server_error
  end

  test 'a sign-in with a nested password does not raise' do
    post '/login', params: { user: { email: 'a@b.test', password: { nested: 'x' } } }

    assert_no_server_error
  end

  # The guard must not have bought its silence by accepting anything: a well-formed attempt
  # with the wrong password still has to fail as one.
  test 'a well-formed attempt with a wrong password still fails to sign in' do
    post '/login', params: { user: { email: 'a@b.test', password: 'wrong-password' } }

    assert_no_server_error
    assert_nil session['warden.user.user.key'], 'a wrong password must not sign anyone in'
  end

  private

  def assert_no_server_error
    assert_not response.server_error?, "expected no 5xx, got #{response.status}"
  end
end
