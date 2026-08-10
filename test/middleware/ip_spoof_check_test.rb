# frozen_string_literal: true

require 'test_helper'

# ActionDispatch::RemoteIp raises IpSpoofAttackError when a request carries both Client-IP
# and a conflicting X-Forwarded-For. That raise happens in Rails::Rack::Logger, which sits
# ABOVE the exception-rendering middleware and above Rack::Attack — so it becomes a 500
# that no throttle can bound, on any path, from an unauthenticated caller sending two
# headers. The check earns nothing here either way: without a declared proxy the app keys
# on REMOTE_ADDR and ignores these headers entirely, and with one it reads the address the
# declared proxy wrote.
class IpSpoofCheckTest < ActionDispatch::IntegrationTest
  SPOOFED = { 'HTTP_CLIENT_IP' => '1.2.3.4', 'HTTP_X_FORWARDED_FOR' => '5.6.7.8' }.freeze

  # Without an admin the app redirects everything to /setup, which would hide whether the
  # request got as far as rendering anything at all.
  setup { create(:user, admin: true, setup_completed: true) }

  test 'conflicting forwarded headers do not raise on an unauthenticated page' do
    get '/en/login', headers: SPOOFED

    assert_response :success
  end

  test 'conflicting forwarded headers do not raise on the health check' do
    get '/health-check', headers: SPOOFED

    assert_response :success
  end

  # The endpoints an unauthenticated caller can reach are the ones worth pinning, since
  # this needs no session at all.
  test 'conflicting forwarded headers do not raise on a POST' do
    post '/login', params: { user: { email: 'a@b.test', password: 'wrong' } }, headers: SPOOFED

    assert_includes 200..499, response.status, 'must not be a 500'
  end
end
