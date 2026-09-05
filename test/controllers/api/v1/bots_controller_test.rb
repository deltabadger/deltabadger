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
    ConnectedClient.create!(
      user: @user, oauth_application: @oauth_app,
      rest_tools: AppConfig::REST_TOOL_DEFAULTS.keys
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
    # connected_clients has a real FK to users, so it has to go first — the point
    # of the test is an orphaned token, not an orphaned grant.
    ConnectedClient.where(user_id: @user.id).delete_all
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

  test 'POST /api/v1/bots schedules a bot for a future start_at' do
    @user.set_rest_tool_enabled('create_bot', true)
    travel_to Time.utc(2026, 5, 26, 12, 0, 0) do
      Bot::ActionJob.stubs(:set).returns(stub(perform_later: true))
      Bot::BroadcastAfterScheduledActionJob.stubs(:perform_later)
      Bot::ActionJob.expects(:perform_later).never
      exchange = create(:binance_exchange)
      btc = create(:asset, :bitcoin)
      usd = create(:asset, :usd)
      create(:ticker, exchange: exchange, base_asset: btc, quote_asset: usd)
      create(:api_key, user: @user, exchange: exchange, key_type: :trading, status: :correct)
      token = create_token(scopes: 'api')

      post '/api/v1/bots',
           params: {
             exchange_name: 'Binance', base_asset: 'BTC', quote_asset: 'USD',
             quote_amount: 100, interval: 'day', start_at: '2026-06-01T09:00:00Z'
           },
           headers: bearer(token), as: :json

      assert_response :created
      body = JSON.parse(response.body)
      assert_equal 'scheduled', body['data']['status']
      assert_equal Time.utc(2026, 6, 1, 9, 0, 0), Time.iso8601(body['data']['started_at'])
    end
  end

  test 'POST /api/v1/bots returns 422 for a past start_at and creates no bot' do
    @user.set_rest_tool_enabled('create_bot', true)
    travel_to Time.utc(2026, 5, 26, 12, 0, 0) do
      exchange = create(:binance_exchange)
      btc = create(:asset, :bitcoin)
      usd = create(:asset, :usd)
      create(:ticker, exchange: exchange, base_asset: btc, quote_asset: usd)
      create(:api_key, user: @user, exchange: exchange, key_type: :trading, status: :correct)
      token = create_token(scopes: 'api')

      assert_no_difference -> { @user.bots.count } do
        post '/api/v1/bots',
             params: {
               exchange_name: 'Binance', base_asset: 'BTC', quote_asset: 'USD',
               quote_amount: 100, interval: 'day', start_at: '2026-05-20T09:00:00Z'
             },
             headers: bearer(token), as: :json
      end

      assert_response :unprocessable_entity
      assert_equal 'bot_invalid', JSON.parse(response.body)['error']['code']
    end
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

  # ---- per-client grants --------------------------------------------------

  test 'refuses a tool the user enabled but this client was not granted' do
    @user.set_rest_tool_enabled('list_bots', true)
    ConnectedClient.find_or_create_by!(user: @user, oauth_application: @oauth_app).update!(rest_tools: [])

    get '/api/v1/bots', headers: bearer(create_token(scopes: 'api'))

    assert_response :forbidden
    assert_equal 'tool_disabled', response.parsed_body.dig('error', 'code')
  end

  test 'allows a tool the user enabled and this client was granted' do
    @user.set_rest_tool_enabled('list_bots', true)
    ConnectedClient.find_or_create_by!(user: @user, oauth_application: @oauth_app).update!(rest_tools: %w[list_bots])

    get '/api/v1/bots', headers: bearer(create_token(scopes: 'api'))

    assert_response :success
  end

  test 'refuses a granted tool the user has turned off' do
    @user.set_rest_tool_enabled('list_bots', false)
    ConnectedClient.find_or_create_by!(user: @user, oauth_application: @oauth_app).update!(rest_tools: %w[list_bots])

    get '/api/v1/bots', headers: bearer(create_token(scopes: 'api'))

    assert_response :forbidden
  end

  test 'refuses everything when the client has no grant record' do
    @user.set_rest_tool_enabled('list_bots', true)
    ConnectedClient.where(user: @user, oauth_application: @oauth_app).delete_all

    get '/api/v1/bots', headers: bearer(create_token(scopes: 'api'))

    assert_response :forbidden
  end

  test 'a second client of the same user gets its own grant' do
    @user.set_rest_tool_enabled('list_bots', true)
    ConnectedClient.find_or_create_by!(user: @user, oauth_application: @oauth_app).update!(rest_tools: %w[list_bots])
    other_app = Doorkeeper::Application.create!(
      name: 'Other', redirect_uri: 'http://localhost/other', confidential: false, scopes: 'api'
    )
    ConnectedClient.create!(user: @user, oauth_application: other_app, rest_tools: [])
    other_token = Doorkeeper::AccessToken.create!(
      application: other_app, resource_owner_id: @user.id,
      token: SecureRandom.hex(32), scopes: 'api', expires_in: 3600
    )

    get '/api/v1/bots', headers: bearer(other_token)

    assert_response :forbidden
  end

  test 'the personal api token is not restricted by a grant record' do
    @user.set_rest_tool_enabled('list_bots', true)

    get '/api/v1/bots', headers: bearer(@user.personal_api_token)

    assert_response :success
  end

  # ---- index bots ---------------------------------------------------------

  test 'POST /api/v1/bots with type index creates a DcaIndex bot' do
    @user.set_rest_tool_enabled('create_index_bot', true)
    with_index_fixture

    post '/api/v1/bots',
         params: { type: 'index', exchange_name: 'Kraken', quote_asset: 'EUR', quote_amount: 50,
                   interval: 'week', index: 'layer-1' },
         headers: bearer(create_token), as: :json

    assert_response :created
    bot = @user.bots.find(JSON.parse(response.body)['data']['id'])
    assert_equal 'Bots::DcaIndex', bot.type
    assert_equal 'layer-1', bot.index_category_id
  end

  test 'POST /api/v1/bots with type index is gated by create_index_bot, not create_bot' do
    @user.set_rest_tool_enabled('create_bot', true)

    post '/api/v1/bots', params: { type: 'index' }, headers: bearer(create_token), as: :json

    assert_response :forbidden
    assert_equal 'tool_disabled', JSON.parse(response.body)['error']['code']
  end

  test 'POST /api/v1/bots with an unknown type is a 422' do
    @user.set_rest_tool_enabled('create_bot', true)
    @user.set_rest_tool_enabled('create_index_bot', true)

    post '/api/v1/bots', params: { type: 'bogus' }, headers: bearer(create_token), as: :json

    assert_response :unprocessable_entity
    assert_equal 'invalid_bot_type', JSON.parse(response.body)['error']['code']
  end

  test 'GET /api/v1/indices lists the indices' do
    @user.set_rest_tool_enabled('list_indices', true)
    with_index_fixture

    get '/api/v1/indices', headers: bearer(create_token)

    assert_response :ok
    body = JSON.parse(response.body)['data']
    assert_equal 1, body['count']
    assert_equal 'layer-1', body['indices'].first['id']
  end

  test 'GET /api/v1/indices is gated' do
    get '/api/v1/indices', headers: bearer(create_token)
    assert_response :forbidden
  end

  test 'POST /api/v1/bots accepts a basket as an array of symbol and allocation' do
    @user.set_rest_tool_enabled('create_bot', true)
    exchange = create(:binance_exchange)
    usd = create(:asset, :usd)
    [create(:asset, :bitcoin), create(:asset, :ethereum)].each do |asset|
      create(:ticker, exchange: exchange, base_asset: asset, quote_asset: usd)
    end
    create(:api_key, user: @user, exchange: exchange, key_type: :trading, status: :correct)
    Bot::ActionJob.stubs(:perform_later)
    Bot::BroadcastAfterScheduledActionJob.stubs(:perform_later)

    post '/api/v1/bots',
         params: { exchange_name: 'Binance', quote_asset: 'USD', quote_amount: 100, interval: 'day',
                   assets: [{ symbol: 'BTC', allocation: 50 }, { symbol: 'ETH', allocation: 50 }] },
         headers: bearer(create_token), as: :json

    assert_response :created
    bot = @user.bots.find(JSON.parse(response.body)['data']['id'])
    assert_equal 'Bots::DcaMultiAsset', bot.type
    assert_equal %w[BTC ETH], bot.base_assets.map(&:symbol)
  end

  test 'PATCH /api/v1/bots/:id reweights a basket from a JSON object' do
    @user.set_rest_tool_enabled('update_bot_settings', true)
    btc = create(:asset, :bitcoin)
    eth = create(:asset, :ethereum)
    bot = create(:dca_multi_asset, :stopped, user: @user, base_assets: [btc, eth])

    patch "/api/v1/bots/#{bot.id}",
          params: { allocations: { BTC: 70, ETH: 30 } },
          headers: bearer(create_token), as: :json

    assert_response :ok
    assert_in_delta 0.7, bot.reload.allocation_for(btc.id), 0.0001
  end

  test 'PATCH /api/v1/bots/:id retunes an index bot' do
    @user.set_rest_tool_enabled('update_bot_settings', true)
    bot = create(:dca_index, user: @user, status: :stopped)

    patch "/api/v1/bots/#{bot.id}",
          params: { num_coins: 8, allocation_flattening: 0.25 },
          headers: bearer(create_token), as: :json

    assert_response :ok
    bot.reload
    assert_equal 8, bot.num_coins
    assert_equal 0.25, bot.allocation_flattening
  end

  # ---- delete / archive / reactivate ---------------------------------------

  test 'DELETE /api/v1/bots/:id deletes a running bot and cancels its tick' do
    @user.set_rest_tool_enabled('delete_bot', true)
    bot = create(:dca_single_asset, user: @user, status: :scheduled, started_at: Time.current)
    Bots::DcaSingleAsset.any_instance.expects(:cancel_scheduled_action_jobs)

    delete "/api/v1/bots/#{bot.id}", headers: bearer(create_token)

    assert_response :ok
    assert bot.reload.deleted?
  end

  test 'POST and DELETE /api/v1/bots/:id/archive archive and reactivate' do
    @user.set_rest_tool_enabled('archive_bot', true)
    @user.set_rest_tool_enabled('unarchive_bot', true)
    bot = create(:dca_single_asset, :stopped, user: @user)

    post "/api/v1/bots/#{bot.id}/archive", headers: bearer(create_token)
    assert_response :ok
    assert bot.reload.archived?

    delete "/api/v1/bots/#{bot.id}/archive", headers: bearer(create_token)
    assert_response :ok
    assert bot.reload.stopped?
  end

  test 'archiving twice is a 409' do
    @user.set_rest_tool_enabled('archive_bot', true)
    bot = create(:dca_single_asset, :stopped, user: @user)

    post "/api/v1/bots/#{bot.id}/archive", headers: bearer(create_token)
    post "/api/v1/bots/#{bot.id}/archive", headers: bearer(create_token)

    assert_response :conflict
    assert_equal 'bot_archived', JSON.parse(response.body)['error']['code']
  end

  test 'the three lifecycle actions are gated by their own tools' do
    bot = create(:dca_single_asset, :stopped, user: @user)
    token = create_token

    delete "/api/v1/bots/#{bot.id}", headers: bearer(token)
    assert_response :forbidden
    post "/api/v1/bots/#{bot.id}/archive", headers: bearer(token)
    assert_response :forbidden
    delete "/api/v1/bots/#{bot.id}/archive", headers: bearer(token)
    assert_response :forbidden
  end

  private

  # A Kraken venue with three EUR pairs — the minimum a category index needs to offer that quote.
  def with_index_fixture
    exchange = create(:kraken_exchange)
    eur = create(:asset, :eur)
    [create(:asset, :bitcoin), create(:asset, :ethereum),
     create(:asset, symbol: 'SOL', name: 'Solana', external_id: 'solana', category: 'Cryptocurrency')]
      .each { |asset| create(:ticker, exchange: exchange, base_asset: asset, quote_asset: eur) }
    create(:api_key, user: @user, exchange: exchange, key_type: :trading, status: :correct)
    create(:index, external_id: 'layer-1', source: Index::SOURCE_COINGECKO, name: 'Layer 1',
                   top_coins: %w[bitcoin ethereum solana], available_exchanges: { 'Exchanges::Kraken' => 3 })
    MarketData.stubs(:configured?).returns(true)
    MarketDataSettings.stubs(:deltabadger?).returns(true)
    MarketData.stubs(:get_top_coins).returns(Result::Success.new(%w[bitcoin ethereum solana]))
    Bot::ActionJob.stubs(:perform_later)
    Bot::BroadcastAfterScheduledActionJob.stubs(:perform_later)
  end

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
