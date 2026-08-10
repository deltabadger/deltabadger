# frozen_string_literal: true

require 'test_helper'

class RackAttackTest < ActionDispatch::IntegrationTest
  setup do
    # Every test below spends its requests against one throttle counter, and rack-attack
    # counts into a window aligned to the wall clock rather than to the first request:
    # Rack::Attack::Cache#key_and_expiry keys each counter on (Time.now.to_i / period), and
    # all eight rules use period: 60. A test whose requests straddle a minute boundary is
    # therefore counted as two short runs, neither of which reaches the limit, and it fails
    # having done nothing wrong. Freezing the clock mid-window puts every request in one
    # bucket. It is the window these tests need pinned, not one they are measuring — none of
    # them asserts anything about a counter expiring — so this changes no assertion.
    travel_to Time.at(((Time.now.to_i / 60) * 60) + 30)
    @original_store = Rack::Attack.cache.store
    @original_enabled = Rack::Attack.enabled
    Rack::Attack.cache.store = ActiveSupport::Cache::MemoryStore.new
    Rack::Attack.reset!
    Rack::Attack.enabled = true
    create(:user, admin: true, setup_completed: true)
  end

  teardown do
    Rack::Attack.cache.store = @original_store
    Rack::Attack.enabled = @original_enabled
  end

  test 'throttles POST /login' do
    11.times { post '/login', params: { user: { email: 'a@b.test', password: 'wrong' } } }
    assert_response :too_many_requests
  end

  # Before the throttles matched real routed paths the login rule never fired, so
  # rack-attack's bare default response was unreachable. It reaches humans on an auth page
  # now, and it has to be the same response whatever tripped it — a rule that named the
  # endpoint would tell an attacker which of their guesses was worth repeating.
  test 'a throttled request says what happened' do
    11.times { post '/login', params: { user: { email: 'a@b.test', password: 'wrong' } } }

    assert_response :too_many_requests
    assert_equal I18n.t('errors.throttled'), response.body.strip
    assert response.headers['retry-after'].to_i.positive?, 'the response must say how long to wait'
    login_body = response.body

    6.times { post '/password', params: { user: { email: 'a@b.test' } } }

    assert_response :too_many_requests
    assert_equal login_body, response.body, 'the response must not identify which rule tripped'
  end

  test 'throttles POST /en/login under the locale scope' do
    11.times { post '/en/login', params: { user: { email: 'a@b.test', password: 'wrong' } } }
    assert_response :too_many_requests
  end

  # Every locale the app ships routes, so every locale has to be covered. Asserting the
  # whole set rather than one example: the list the patterns are built from is not this
  # app's list unless it is read from the right place, and a hole is invisible until
  # someone tries the locale it left out.
  test 'no shipped locale escapes the login throttle' do
    I18n.available_locales.each do |locale|
      assert RackAttackPaths::LOGIN.match?("/#{locale}/login"),
             "POST /#{locale}/login is a routed path the login throttle does not match"
    end
  end

  test 'throttles POST /verify_two_factor' do
    6.times { post '/verify_two_factor', params: { user: { otp_code_token: '000000' } } }
    assert_response :too_many_requests
  end

  test 'throttles POST /pl/verify_two_factor under the locale scope' do
    6.times { post '/pl/verify_two_factor', params: { user: { otp_code_token: '000000' } } }
    assert_response :too_many_requests
  end

  # POST /password requests a reset email; PATCH/PUT /password submits the new password
  # and the OTP. The form uses PATCH (app/views/devise/passwords/edit.html.erb:5), so a
  # POST-only rule would leave reset-token and OTP guessing unthrottled.
  test 'throttles POST /password (reset request)' do
    6.times { post '/password', params: { user: { email: 'a@b.test' } } }
    assert_response :too_many_requests
  end

  test 'throttles PATCH /password (reset submission)' do
    6.times do
      patch '/password', params: { user: { reset_password_token: 'x', password: 'Aa1!aaaaaaaa',
                                           password_confirmation: 'Aa1!aaaaaaaa' } }
    end
    assert_response :too_many_requests
  end

  test 'throttles PUT /password (reset submission)' do
    6.times do
      put '/password', params: { user: { reset_password_token: 'x', password: 'Aa1!aaaaaaaa',
                                         password_confirmation: 'Aa1!aaaaaaaa' } }
    end
    assert_response :too_many_requests
  end

  test 'throttles POST /setup' do
    User.delete_all
    6.times { post '/setup', params: { user: { name: 'A', email: 'a@b.test', password: 'Aa1!aaaaaaaa' } } }
    assert_response :too_many_requests
  end

  test 'throttles POST /oauth/token' do
    21.times { post '/oauth/token', params: { grant_type: 'authorization_code', code: 'x' } }
    assert_response :too_many_requests
  end

  test 'throttles GET /oauth/authorize' do
    11.times { get '/oauth/authorize', params: { client_id: 'x', response_type: 'code' } }
    assert_response :too_many_requests
  end

  test 'a format suffix does not bypass the oauth/register throttle' do
    6.times do
      post '/oauth/register.json', params: { redirect_uris: ['https://e.test/cb'] }, as: :json
    end
    assert_response :too_many_requests
  end

  test 'a non-alphanumeric format suffix does not bypass it either' do
    6.times do
      post '/oauth/register.json-api', params: { redirect_uris: ['https://e.test/cb'] }, as: :json
    end
    assert_response :too_many_requests
  end

  # Each suffix must land in the SAME bucket, or an attacker just varies the suffix.
  test 'suffixed and unsuffixed requests share one bucket' do
    3.times { post '/oauth/register', params: { redirect_uris: ['https://e.test/cb'] }, as: :json }
    3.times { post '/oauth/register.json', params: { redirect_uris: ['https://e.test/cb'] }, as: :json }
    assert_response :too_many_requests
  end

  # Verified with recognize_path: Rails routes all of these to the same action, so a
  # regex over the raw path would let one extra slash bypass every throttle.
  test 'a trailing slash does not bypass the login throttle' do
    11.times { post '/login/', params: { user: { email: 'a@b.test', password: 'wrong' } } }
    assert_response :too_many_requests
  end

  test 'a doubled slash does not bypass the oauth/register throttle' do
    6.times { post '/oauth//register', params: { redirect_uris: ['https://e.test/cb'] }, as: :json }
    assert_response :too_many_requests
  end

  test 'slash variants share one bucket with the canonical path' do
    2.times { post '/oauth/register', params: { redirect_uris: ['https://e.test/cb'] }, as: :json }
    2.times { post '/oauth/register/', params: { redirect_uris: ['https://e.test/cb'] }, as: :json }
    2.times { post '/oauth//register', params: { redirect_uris: ['https://e.test/cb'] }, as: :json }
    assert_response :too_many_requests
  end

  # The throttle key used to be action_dispatch.remote_ip unconditionally, which prefers a
  # forwarded-for hop over REMOTE_ADDR. On a deployment with nothing in front of it that hop
  # is written by the caller, so the caller chooses its own bucket and the per-IP limit
  # bounds nothing. Keyed on REMOTE_ADDR the bucket is whatever the TCP connection came
  # from, which the caller cannot restate.
  test 'a rotating forwarded-for header does not mint a fresh bucket per request' do
    declare_proxy(false)
    11.times { |i| post_login('REMOTE_ADDR' => '198.18.0.5', 'HTTP_X_FORWARDED_FOR' => "203.0.113.#{i + 1}") }

    assert_response :too_many_requests, 'the limit must hold however the forwarded header varies'
  end

  # Client-Ip reaches calculate_ip by the same route as X-Forwarded-For, so a fix that only
  # closed the better-known header would leave an identical second way in.
  test 'a rotating client-ip header does not mint a fresh bucket per request' do
    declare_proxy(false)
    11.times { |i| post_login('REMOTE_ADDR' => '198.18.0.5', 'HTTP_CLIENT_IP' => "203.0.113.#{i + 1}") }

    assert_response :too_many_requests, 'the limit must hold however the client-ip header varies'
  end

  # The other direction: keying on REMOTE_ADDR must not collapse everyone into one bucket,
  # or the limit becomes a way for one caller to lock out the rest.
  test 'distinct REMOTE_ADDRs keep independent budgets' do
    declare_proxy(false)
    11.times { post_login('REMOTE_ADDR' => '198.18.0.5') }
    assert_response :too_many_requests

    post_login('REMOTE_ADDR' => '198.18.0.6')
    refute_equal 429, response.status, 'a second address must not inherit the first address budget'
  end

  # Behind a proxy that writes the header, the header is the only thing that distinguishes
  # callers — REMOTE_ADDR is the proxy for all of them. This pins that the declared branch
  # still reads it, so hosted and reverse-proxied installs keep per-caller buckets.
  test 'a declared proxy still keys on the forwarded header' do
    declare_proxy(true)
    11.times { post_login('REMOTE_ADDR' => '172.16.0.2', 'HTTP_X_FORWARDED_FOR' => '203.0.113.1') }
    assert_response :too_many_requests

    post_login('REMOTE_ADDR' => '172.16.0.2', 'HTTP_X_FORWARDED_FOR' => '203.0.113.2')
    refute_equal 429, response.status, 'a second forwarded caller must not inherit the first budget'
  end

  # Rack::Attack::Throttle#matched_by? opens with `return false unless discriminator`, so a
  # nil key does not mean "one shared bucket", it means the rule does not run at all. An
  # environment with no REMOTE_ADDR must therefore still produce a key.
  test 'the throttle key is never nil' do
    request = Rack::Attack::Request.new({})

    declare_proxy(false)
    assert Rack::Attack.client_ip(request).present?, 'a missing REMOTE_ADDR must not switch the rule off'

    declare_proxy(true)
    assert Rack::Attack.client_ip(request).present?, 'a missing REMOTE_ADDR must not switch the rule off'
  end

  # /csp-report takes an unauthenticated POST and writes a line to the log, so it is a
  # log-flooding vector. The 3 + 28 split is what proves both spellings land in ONE bucket:
  # against a limit of 30 neither half reaches the limit alone, so a rule matching only the
  # bare path or only the suffixed one would leave this green at 204.
  test 'throttles the CSP report endpoint, format suffix included' do
    3.times { post_csp_report('/csp-report') }
    28.times { post_csp_report('/csp-report.json') }

    assert_response :too_many_requests
  end

  # /password is throttled; /confirmation was not, and it is the same shape — an
  # unauthenticated POST that makes the app send mail to an address the caller names.
  test 'throttles POST /confirmation' do
    6.times { post '/confirmation', params: { user: { email: 'a@b.test' } } }

    assert_response :too_many_requests
  end

  test 'throttles POST /confirmation under a locale prefix' do
    6.times { post '/pl/confirmation', params: { user: { email: 'a@b.test' } } }

    assert_response :too_many_requests
  end

  test 'a format suffix does not bypass the confirmation throttle' do
    3.times { post '/confirmation', params: { user: { email: 'a@b.test' } } }
    3.times { post '/confirmation.json', params: { user: { email: 'a@b.test' } } }

    assert_response :too_many_requests
  end

  private

  def post_csp_report(path)
    post path, params: '{}', headers: { 'CONTENT_TYPE' => 'application/csp-report' }
  end

  # Stub rather than set BEHIND_PROXY: ENV is process-wide and shared with every other test
  # in this process, and mocha restores this in teardown whichever way the test exits.
  def declare_proxy(declared)
    Deltabadger::Application.stubs(:behind_proxy_from_env).returns(declared)
  end

  def post_login(headers)
    post '/login', params: { user: { email: 'a@b.test', password: 'wrong' } }, headers: headers
  end
end
