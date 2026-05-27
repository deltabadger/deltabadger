# frozen_string_literal: true

require 'test_helper'

class Api::V1::BotsControllerTest < ActionDispatch::IntegrationTest
  include Devise::Test::IntegrationHelpers

  setup do
    @user = create(:user)
    @oauth_app = Doorkeeper::Application.create!(
      name: 'Test API Client',
      redirect_uri: 'http://localhost/callback',
      confidential: false,
      scopes: 'api mcp'
    )
  end

  teardown do
    Doorkeeper::AccessToken.delete_all
    Doorkeeper::Application.destroy_all
  end

  # ---- happy path ---------------------------------------------------------

  test 'returns 200 with bots when list_bots is enabled and token has api scope' do
    @user.set_rest_tool_enabled('list_bots', true)
    create(:dca_single_asset, user: @user, status: :scheduled, started_at: Time.current)
    token = create_token(scopes: 'api')

    get '/api/v1/bots', headers: bearer(token)

    assert_response :ok
    json = JSON.parse(response.body)
    assert_nil json['error']
    assert_kind_of Hash, json['data']
    assert_equal 1, json['data']['count']
    assert_equal 1, json['data']['bots'].size

    row = json['data']['bots'].first
    %w[id label type pair exchange status interval quote_amount].each do |key|
      assert row.key?(key), "expected row to include #{key}"
    end
  end

  test 'returns count 0 and empty array when user has no bots' do
    @user.set_rest_tool_enabled('list_bots', true)
    token = create_token(scopes: 'api')

    get '/api/v1/bots', headers: bearer(token)

    assert_response :ok
    json = JSON.parse(response.body)
    assert_equal 0, json['data']['count']
    assert_equal [], json['data']['bots']
  end

  test 'forwards status filter to the service' do
    @user.set_rest_tool_enabled('list_bots', true)
    btc = create(:asset, :bitcoin)
    eth = create(:asset, :ethereum)
    usd = create(:asset, :usd)
    exchange = create(:binance_exchange)
    create(:dca_single_asset, user: @user, status: :scheduled, started_at: Time.current,
                              base_asset: btc, quote_asset: usd, exchange: exchange)
    create(:dca_single_asset, :stopped, user: @user, base_asset: eth, quote_asset: usd, exchange: exchange)
    token = create_token(scopes: 'api')

    get '/api/v1/bots', params: { status: 'scheduled' }, headers: bearer(token)

    assert_response :ok
    json = JSON.parse(response.body)
    assert_equal 1, json['data']['count']
    assert_equal(['scheduled'], json['data']['bots'].map { |b| b['status'] })
  end

  # ---- default-denied -----------------------------------------------------

  test 'returns 403 with tool_disabled when list_bots is off (default) even with a valid api token' do
    # No `set_rest_tool_enabled` call — REST defaults are all-off.
    token = create_token(scopes: 'api')

    get '/api/v1/bots', headers: bearer(token)

    assert_response :forbidden
    json = JSON.parse(response.body)
    assert_nil json['data']
    assert_equal 'tool_disabled', json['error']['code']
    assert_includes json['error']['message'], 'list_bots'
  end

  test 'disabling list_bots after enabling it returns 403 again' do
    @user.set_rest_tool_enabled('list_bots', true)
    @user.set_rest_tool_enabled('list_bots', false)
    token = create_token(scopes: 'api')

    get '/api/v1/bots', headers: bearer(token)

    assert_response :forbidden
    assert_equal 'tool_disabled', JSON.parse(response.body)['error']['code']
  end

  # ---- auth failures -----------------------------------------------------

  test 'returns 401 when no Authorization header is sent' do
    get '/api/v1/bots'

    assert_response :unauthorized
    json = JSON.parse(response.body)
    assert_nil json['data']
    assert_equal 'missing_token', json['error']['code']
  end

  test 'returns 401 when bearer token is unknown' do
    get '/api/v1/bots', headers: { 'Authorization' => 'Bearer bogus_token_value' }

    assert_response :unauthorized
    assert_equal 'invalid_token', JSON.parse(response.body)['error']['code']
  end

  test 'returns 401 when bearer token is revoked' do
    token = create_token(scopes: 'api', revoked_at: Time.current)
    get '/api/v1/bots', headers: bearer(token)

    assert_response :unauthorized
    assert_equal 'token_revoked', JSON.parse(response.body)['error']['code']
  end

  test 'returns 401 when bearer token is expired' do
    token = create_token(scopes: 'api', expires_in: 0, created_at: 1.hour.ago)
    get '/api/v1/bots', headers: bearer(token)

    assert_response :unauthorized
    assert_equal 'token_expired', JSON.parse(response.body)['error']['code']
  end

  test 'returns 403 with insufficient_scope when token has only mcp scope' do
    @user.set_rest_tool_enabled('list_bots', true) # tool enabled, but scope wrong
    token = create_token(scopes: 'mcp')

    get '/api/v1/bots', headers: bearer(token)

    assert_response :forbidden
    assert_equal 'insufficient_scope', JSON.parse(response.body)['error']['code']
  end

  test 'accepts a token with multiple scopes including api' do
    @user.set_rest_tool_enabled('list_bots', true)
    token = create_token(scopes: 'api mcp')

    get '/api/v1/bots', headers: bearer(token)

    assert_response :ok
  end

  test 'returns 401 user_not_found when the bearer token outlives its user' do
    token = create_token(scopes: 'api')
    # Bypass dependent: :destroy so the token survives but the user is gone.
    User.where(id: @user.id).delete_all

    get '/api/v1/bots', headers: bearer(token)

    assert_response :unauthorized
    assert_equal 'user_not_found', JSON.parse(response.body)['error']['code']
  end

  # ---- session-auth must not fall back through ----------------------------

  test 'rejects a browser session (no bearer token) — REST is OAuth-only' do
    # The plan explicitly forbids session-auth fallback at /api/v1/*.
    # Without a bearer header, the request must look identical to an
    # unauthenticated one regardless of any active web session.
    sign_in @user
    @user.set_rest_tool_enabled('list_bots', true)

    get '/api/v1/bots'

    assert_response :unauthorized
    assert_equal 'missing_token', JSON.parse(response.body)['error']['code']
  end

  test 'browser session with an invalid bearer header is rejected as invalid_token, not authenticated' do
    sign_in @user
    @user.set_rest_tool_enabled('list_bots', true)

    get '/api/v1/bots', headers: { 'Authorization' => 'Bearer wrong' }

    assert_response :unauthorized
    assert_equal 'invalid_token', JSON.parse(response.body)['error']['code']
  end

  # ---- response envelope --------------------------------------------------

  test 'response is application/json' do
    @user.set_rest_tool_enabled('list_bots', true)
    token = create_token(scopes: 'api')

    get '/api/v1/bots', headers: bearer(token)

    assert_match(%r{application/json}, response.headers['Content-Type'])
  end

  test 'success envelope is { data: <hash>, error: null }' do
    @user.set_rest_tool_enabled('list_bots', true)
    token = create_token(scopes: 'api')

    get '/api/v1/bots', headers: bearer(token)

    json = JSON.parse(response.body)
    assert json.key?('data')
    assert json.key?('error')
    assert_nil json['error']
  end

  test 'error envelope is { data: null, error: { code, message } }' do
    token = create_token(scopes: 'api') # tool disabled by default → 403

    get '/api/v1/bots', headers: bearer(token)

    json = JSON.parse(response.body)
    assert_nil json['data']
    assert json['error'].is_a?(Hash)
    assert json['error'].key?('code')
    assert json['error'].key?('message')
  end

  # ---- show (get_bot_details) ---------------------------------------------

  test 'GET /api/v1/bots/:id returns the bot detail when get_bot_details is enabled' do
    @user.set_rest_tool_enabled('get_bot_details', true)
    bot = create(:dca_single_asset, user: @user, status: :scheduled, started_at: Time.current)
    Bots::DcaSingleAsset.any_instance.stubs(:metrics).returns(nil)
    token = create_token(scopes: 'api')

    get "/api/v1/bots/#{bot.id}", headers: bearer(token)

    assert_response :ok
    body = JSON.parse(response.body)
    assert_equal bot.id, body['data']['id']
    assert_equal 'scheduled', body['data']['status']
    assert_nil body['data']['metrics']
  end

  test 'GET /api/v1/bots/:id returns 404 bot_not_found for an unknown id' do
    @user.set_rest_tool_enabled('get_bot_details', true)
    token = create_token(scopes: 'api')

    get '/api/v1/bots/999999', headers: bearer(token)

    assert_response :not_found
    assert_equal 'bot_not_found', JSON.parse(response.body)['error']['code']
  end

  test 'GET /api/v1/bots/:id returns 403 tool_disabled when get_bot_details is off' do
    bot = create(:dca_single_asset, user: @user, status: :scheduled, started_at: Time.current)
    token = create_token(scopes: 'api')

    get "/api/v1/bots/#{bot.id}", headers: bearer(token)

    assert_response :forbidden
    assert_equal 'tool_disabled', JSON.parse(response.body)['error']['code']
  end

  # ---- start --------------------------------------------------------------

  test 'POST /api/v1/bots/:id/start starts the bot when start_bot is enabled' do
    @user.set_rest_tool_enabled('start_bot', true)
    Bot::ActionJob.stubs(:perform_later)
    Bot::BroadcastAfterScheduledActionJob.stubs(:perform_later)
    bot = create(:dca_single_asset, user: @user, status: :created)
    token = create_token(scopes: 'api')

    post "/api/v1/bots/#{bot.id}/start", headers: bearer(token)

    assert_response :ok
    assert_equal bot.id, JSON.parse(response.body)['data']['id']
    assert bot.reload.working?
  end

  test 'POST /api/v1/bots/:id/start returns 409 when bot is already running' do
    @user.set_rest_tool_enabled('start_bot', true)
    bot = create(:dca_single_asset, user: @user, status: :scheduled, started_at: Time.current)
    token = create_token(scopes: 'api')

    post "/api/v1/bots/#{bot.id}/start", headers: bearer(token)

    assert_response :conflict
    assert_equal 'bot_already_running', JSON.parse(response.body)['error']['code']
  end

  test 'POST /api/v1/bots/:id/start is 403 tool_disabled by default' do
    bot = create(:dca_single_asset, user: @user, status: :created)
    token = create_token(scopes: 'api')

    post "/api/v1/bots/#{bot.id}/start", headers: bearer(token)

    assert_response :forbidden
    assert_equal 'tool_disabled', JSON.parse(response.body)['error']['code']
  end

  # ---- stop ---------------------------------------------------------------

  test 'POST /api/v1/bots/:id/stop stops a running bot when stop_bot is enabled' do
    @user.set_rest_tool_enabled('stop_bot', true)
    bot = create(:dca_single_asset, user: @user, status: :scheduled, started_at: Time.current)
    token = create_token(scopes: 'api')

    post "/api/v1/bots/#{bot.id}/stop", headers: bearer(token)

    assert_response :ok
    assert bot.reload.stopped?
  end

  test 'POST /api/v1/bots/:id/stop returns 409 when bot is not running' do
    @user.set_rest_tool_enabled('stop_bot', true)
    bot = create(:dca_single_asset, user: @user, status: :created)
    token = create_token(scopes: 'api')

    post "/api/v1/bots/#{bot.id}/stop", headers: bearer(token)

    assert_response :conflict
    assert_equal 'bot_not_running', JSON.parse(response.body)['error']['code']
  end

  # ---- update -------------------------------------------------------------

  test 'PATCH /api/v1/bots/:id updates label and quote_amount on a stopped bot' do
    @user.set_rest_tool_enabled('update_bot_settings', true)
    bot = create(:dca_single_asset, user: @user, status: :created)
    token = create_token(scopes: 'api')

    patch "/api/v1/bots/#{bot.id}",
          params: { quote_amount: 250.0, label: 'New Label' },
          headers: bearer(token), as: :json

    assert_response :ok
    body = JSON.parse(response.body)
    assert_equal 'New Label', body['data']['label']
    assert_equal %w[label quote_amount].sort, body['data']['updated'].sort
    bot.reload
    assert_equal 250.0, bot.settings['quote_amount']
    assert_equal 'New Label', bot.label
  end

  test 'PATCH /api/v1/bots/:id with no updatable params returns 422' do
    @user.set_rest_tool_enabled('update_bot_settings', true)
    bot = create(:dca_single_asset, user: @user, status: :created)
    token = create_token(scopes: 'api')

    patch "/api/v1/bots/#{bot.id}", params: {}, headers: bearer(token), as: :json

    assert_response :unprocessable_entity
    assert_equal 'no_updates_provided', JSON.parse(response.body)['error']['code']
  end

  test 'PATCH /api/v1/bots/:id returns 409 when bot is running' do
    @user.set_rest_tool_enabled('update_bot_settings', true)
    bot = create(:dca_single_asset, user: @user, status: :scheduled, started_at: Time.current)
    token = create_token(scopes: 'api')

    patch "/api/v1/bots/#{bot.id}",
          params: { quote_amount: 250.0 },
          headers: bearer(token), as: :json

    assert_response :conflict
    assert_equal 'bot_running', JSON.parse(response.body)['error']['code']
  end

  # ---- create -------------------------------------------------------------

  test 'POST /api/v1/bots creates and starts a single-asset bot' do
    @user.set_rest_tool_enabled('create_bot', true)
    Bot::ActionJob.stubs(:perform_later)
    Bot::BroadcastAfterScheduledActionJob.stubs(:perform_later)
    exchange = create(:binance_exchange)
    btc = create(:asset, :bitcoin)
    usd = create(:asset, :usd)
    create(:ticker, exchange: exchange, base_asset: btc, quote_asset: usd)
    create(:api_key, user: @user, exchange: exchange, key_type: :trading, status: :correct)
    token = create_token(scopes: 'api')

    post '/api/v1/bots',
         params: {
           exchange_name: 'Binance', base_asset: 'BTC', quote_asset: 'USD',
           quote_amount: 100, interval: 'day'
         },
         headers: bearer(token), as: :json

    assert_response :created
    body = JSON.parse(response.body)
    assert_equal 'Bots::DcaSingleAsset', body['data']['type']
    assert_equal 'BTC/USD', body['data']['pair']
    bot = @user.bots.last
    assert bot.working?
  end

  test 'POST /api/v1/bots returns 404 when the exchange is unknown' do
    @user.set_rest_tool_enabled('create_bot', true)
    token = create_token(scopes: 'api')

    post '/api/v1/bots',
         params: {
           exchange_name: 'Nope', base_asset: 'BTC', quote_asset: 'USD',
           quote_amount: 100, interval: 'day'
         },
         headers: bearer(token), as: :json

    assert_response :not_found
    assert_equal 'exchange_not_found', JSON.parse(response.body)['error']['code']
  end

  test 'POST /api/v1/bots returns 422 for invalid interval' do
    @user.set_rest_tool_enabled('create_bot', true)
    exchange = create(:binance_exchange)
    btc = create(:asset, :bitcoin)
    usd = create(:asset, :usd)
    create(:ticker, exchange: exchange, base_asset: btc, quote_asset: usd)
    create(:api_key, user: @user, exchange: exchange, key_type: :trading, status: :correct)
    token = create_token(scopes: 'api')

    post '/api/v1/bots',
         params: {
           exchange_name: 'Binance', base_asset: 'BTC', quote_asset: 'USD',
           quote_amount: 100, interval: 'minute'
         },
         headers: bearer(token), as: :json

    assert_response :unprocessable_entity
    assert_equal 'invalid_interval', JSON.parse(response.body)['error']['code']
  end

  test 'POST /api/v1/bots returns 422 missing_required_parameter when body is empty' do
    @user.set_rest_tool_enabled('create_bot', true)
    token = create_token(scopes: 'api')

    post '/api/v1/bots', params: {}, headers: bearer(token), as: :json

    assert_response :unprocessable_entity
    body = JSON.parse(response.body)
    assert_equal 'missing_required_parameter', body['error']['code']
    # The error message must enumerate all five required fields so the
    # client knows what to send next without a guessing game.
    %w[exchange_name base_asset quote_asset quote_amount interval].each do |field|
      assert_includes body['error']['message'], field
    end
  end

  test 'POST /api/v1/bots returns 422 missing_required_parameter when only some required fields are present' do
    @user.set_rest_tool_enabled('create_bot', true)
    token = create_token(scopes: 'api')

    # Send a partial body — exchange_name + interval missing. This is the
    # exact regression that previously raised ArgumentError before reaching
    # the service.
    post '/api/v1/bots',
         params: { base_asset: 'BTC', quote_asset: 'USD', quote_amount: 100 },
         headers: bearer(token), as: :json

    assert_response :unprocessable_entity
    body = JSON.parse(response.body)
    assert_equal 'missing_required_parameter', body['error']['code']
    assert_includes body['error']['message'], 'exchange_name'
    assert_includes body['error']['message'], 'interval'
    # Fields that *were* provided shouldn't appear in the missing list.
    assert_not_includes body['error']['message'], 'base_asset'
  end

  test 'POST /api/v1/bots is 403 tool_disabled by default' do
    token = create_token(scopes: 'api')

    post '/api/v1/bots',
         params: {
           exchange_name: 'Binance', base_asset: 'BTC', quote_asset: 'USD',
           quote_amount: 100, interval: 'day'
         },
         headers: bearer(token), as: :json

    assert_response :forbidden
    assert_equal 'tool_disabled', JSON.parse(response.body)['error']['code']
  end

  private

  def bearer(token)
    { 'Authorization' => "Bearer #{token.token}" }
  end

  def create_token(scopes: 'api', **attrs)
    Doorkeeper::AccessToken.create!({
      application: @oauth_app,
      resource_owner_id: @user.id,
      token: SecureRandom.hex(32),
      scopes: scopes,
      expires_in: 3600
    }.merge(attrs))
  end
end
