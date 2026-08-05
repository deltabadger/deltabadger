# frozen_string_literal: true

require 'test_helper'

class RackAttackTest < ActionDispatch::IntegrationTest
  setup do
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

  test 'throttles POST /en/login under the locale scope' do
    11.times { post '/en/login', params: { user: { email: 'a@b.test', password: 'wrong' } } }
    assert_response :too_many_requests
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
end
