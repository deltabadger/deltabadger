# frozen_string_literal: true

require 'test_helper'

class Api::V1::ExchangesControllerTest < ActionDispatch::IntegrationTest
  setup do
    @user = create(:user)
    @oauth_app = Doorkeeper::Application.create!(
      name: 'Test', redirect_uri: 'http://localhost/callback',
      confidential: false, scopes: 'api'
    )
  end

  teardown do
    Doorkeeper::AccessToken.delete_all
    Doorkeeper::Application.destroy_all
  end

  # ---- index --------------------------------------------------------------

  test 'GET /api/v1/exchanges returns the user trading exchanges' do
    @user.set_rest_tool_enabled('list_exchanges', true)
    binance = create(:binance_exchange)
    kraken = create(:kraken_exchange)
    create(:api_key, user: @user, exchange: binance, key_type: :trading, status: :correct)
    create(:api_key, user: @user, exchange: kraken, key_type: :trading, status: :incorrect)
    token = api_token

    get '/api/v1/exchanges', headers: bearer(token)

    assert_response :ok
    body = JSON.parse(response.body)
    assert_equal 2, body['data']['count']
    names = body['data']['exchanges'].map { |row| row['name'] }
    assert_includes names, 'Binance'
    assert_includes names, 'Kraken'
    statuses = body['data']['exchanges'].to_h { |row| [row['name'], row['api_key_status']] }
    assert_equal 'correct', statuses['Binance']
    assert_equal 'incorrect', statuses['Kraken']
  end

  test 'GET /api/v1/exchanges returns count 0 when user has no trading keys' do
    @user.set_rest_tool_enabled('list_exchanges', true)
    token = api_token

    get '/api/v1/exchanges', headers: bearer(token)

    assert_response :ok
    body = JSON.parse(response.body)
    assert_equal 0, body['data']['count']
    assert_equal [], body['data']['exchanges']
  end

  test 'GET /api/v1/exchanges is 403 tool_disabled by default' do
    token = api_token
    get '/api/v1/exchanges', headers: bearer(token)
    assert_response :forbidden
    assert_equal 'tool_disabled', JSON.parse(response.body)['error']['code']
  end

  # ---- balances -----------------------------------------------------------

  test 'GET /api/v1/exchanges/:id/balances returns non-zero balances' do
    @user.set_rest_tool_enabled('get_exchange_balances', true)
    exchange = create(:binance_exchange)
    create(:api_key, user: @user, exchange: exchange, key_type: :trading, status: :correct)
    btc = create(:asset, :bitcoin)
    eth = create(:asset, :ethereum)
    upstream = mock
    upstream.expects(:success?).returns(true)
    upstream.expects(:data).returns(btc.id => { free: 1.5, locked: 0.1 },
                                    eth.id => { free: 0.0, locked: 0.0 })
    Exchanges::Binance.any_instance.expects(:set_client).returns(true)
    Exchanges::Binance.any_instance.expects(:get_balances).returns(upstream)
    token = api_token

    get "/api/v1/exchanges/#{exchange.id}/balances", headers: bearer(token)

    assert_response :ok
    body = JSON.parse(response.body)
    assert_equal 'Binance', body['data']['exchange']
    assert_equal 1, body['data']['count']
    row = body['data']['balances'].first
    assert_equal 'BTC', row['symbol']
    assert_equal 1.5, row['free']
    assert_equal 0.1, row['locked']
  end

  test 'GET /api/v1/exchanges/:id/balances returns 404 for unknown exchange' do
    @user.set_rest_tool_enabled('get_exchange_balances', true)
    token = api_token

    get '/api/v1/exchanges/999999/balances', headers: bearer(token)

    assert_response :not_found
    assert_equal 'exchange_not_found', JSON.parse(response.body)['error']['code']
  end

  test 'GET /api/v1/exchanges/:id/balances returns 403 api_key_missing without a valid trading key' do
    @user.set_rest_tool_enabled('get_exchange_balances', true)
    exchange = create(:binance_exchange)
    token = api_token

    get "/api/v1/exchanges/#{exchange.id}/balances", headers: bearer(token)

    assert_response :forbidden
    assert_equal 'api_key_missing', JSON.parse(response.body)['error']['code']
  end

  test 'GET /api/v1/exchanges/:id/balances returns 502 when upstream fetch fails' do
    @user.set_rest_tool_enabled('get_exchange_balances', true)
    exchange = create(:binance_exchange)
    create(:api_key, user: @user, exchange: exchange, key_type: :trading, status: :correct)
    upstream = mock
    upstream.expects(:success?).returns(false)
    upstream.expects(:errors).returns(['rate limited'])
    Exchanges::Binance.any_instance.expects(:set_client).returns(true)
    Exchanges::Binance.any_instance.expects(:get_balances).returns(upstream)
    token = api_token

    get "/api/v1/exchanges/#{exchange.id}/balances", headers: bearer(token)

    assert_response :bad_gateway
    body = JSON.parse(response.body)
    assert_equal 'balances_fetch_failed', body['error']['code']
    assert_includes body['error']['message'], 'rate limited'
  end

  test 'GET /api/v1/exchanges/:id/balances is 403 tool_disabled by default' do
    exchange = create(:binance_exchange)
    token = api_token

    get "/api/v1/exchanges/#{exchange.id}/balances", headers: bearer(token)

    assert_response :forbidden
    assert_equal 'tool_disabled', JSON.parse(response.body)['error']['code']
  end

  private

  def bearer(token)
    { 'Authorization' => "Bearer #{token.token}" }
  end

  def api_token
    Doorkeeper::AccessToken.create!(
      application: @oauth_app, resource_owner_id: @user.id,
      token: SecureRandom.hex(32), scopes: 'api', expires_in: 3600
    )
  end
end
