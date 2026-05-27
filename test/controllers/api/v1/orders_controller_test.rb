# frozen_string_literal: true

require 'test_helper'

class Api::V1::OrdersControllerTest < ActionDispatch::IntegrationTest
  setup do
    @user = create(:user)
    @oauth_app = Doorkeeper::Application.create!(
      name: 'Test', redirect_uri: 'http://localhost/callback',
      confidential: false, scopes: 'api'
    )
    @exchange = create(:binance_exchange)
    @btc = create(:asset, :bitcoin)
    @usd = create(:asset, :usd)
    @ticker = create(:ticker, exchange: @exchange, base_asset: @btc, quote_asset: @usd)
    @api_key = create(:api_key, user: @user, exchange: @exchange, key_type: :trading, status: :correct)
  end

  teardown do
    Doorkeeper::AccessToken.delete_all
    Doorkeeper::Application.destroy_all
    IdempotencyKey.delete_all
  end

  # ---- GET /api/v1/orders (no idempotency expected) -----------------------

  test 'GET /api/v1/orders returns count 0 when no open orders exist' do
    @user.set_rest_tool_enabled('list_open_orders', true)
    token = api_token

    get '/api/v1/orders', headers: bearer(token)

    assert_response :ok
    body = JSON.parse(response.body)
    assert_equal 0, body['data']['count']
  end

  test 'GET /api/v1/orders does NOT require Idempotency-Key' do
    @user.set_rest_tool_enabled('list_open_orders', true)
    token = api_token

    get '/api/v1/orders', headers: bearer(token)

    # 200 (not 400 idempotency_key_required) confirms the read endpoint is
    # unwrapped.
    assert_response :ok
  end

  test 'GET /api/v1/orders is 403 tool_disabled by default' do
    token = api_token
    get '/api/v1/orders', headers: bearer(token)
    assert_response :forbidden
    assert_equal 'tool_disabled', JSON.parse(response.body)['error']['code']
  end

  # ---- POST /api/v1/orders ------------------------------------------------

  test 'POST /api/v1/orders without Idempotency-Key returns 400 before touching the exchange' do
    @user.set_rest_tool_enabled('market_buy', true)
    Exchanges::Binance.any_instance.expects(:market_buy).never
    token = api_token

    post '/api/v1/orders',
         params: order_params(type: 'market_buy'),
         headers: bearer(token), as: :json

    assert_response :bad_request
    assert_equal 'idempotency_key_required', JSON.parse(response.body)['error']['code']
    assert_equal 0, IdempotencyKey.count
  end

  test 'POST /api/v1/orders happy path places an order and stores the idempotency row' do
    @user.set_rest_tool_enabled('market_buy', true)
    stub_exchange_success
    token = api_token

    post '/api/v1/orders',
         params: order_params(type: 'market_buy'),
         headers: bearer(token).merge('Idempotency-Key' => 'k1'), as: :json

    assert_response :created
    body = JSON.parse(response.body)
    assert_equal 'buy', body['data']['side']
    assert_equal 'market', body['data']['order_type']

    record = IdempotencyKey.find_by!(user: @user, key: 'k1')
    assert record.completed?
    assert_equal 201, record.response_status
    assert_equal response.body, record.response_body
  end

  test 'POST /api/v1/orders replay: same key + same fingerprint returns stored bytes without re-hitting exchange' do
    @user.set_rest_tool_enabled('market_buy', true)
    Exchanges::Binance.any_instance.expects(:set_client).once.returns(true)
    Exchanges::Binance.any_instance.expects(:market_buy).once.returns(stub(success?: true, data: 'order_abc'))
    token = api_token

    post '/api/v1/orders',
         params: order_params(type: 'market_buy'),
         headers: bearer(token).merge('Idempotency-Key' => 'replayme'), as: :json
    first_body = response.body
    first_status = response.status

    post '/api/v1/orders',
         params: order_params(type: 'market_buy'),
         headers: bearer(token).merge('Idempotency-Key' => 'replayme'), as: :json

    assert_equal first_status, response.status
    assert_equal first_body, response.body
  end

  test 'POST /api/v1/orders same key + different body returns 409 idempotency_key_reused' do
    @user.set_rest_tool_enabled('market_buy', true)
    stub_exchange_success # only the first attempt should reach the exchange
    token = api_token

    post '/api/v1/orders',
         params: order_params(type: 'market_buy', amount: 100),
         headers: bearer(token).merge('Idempotency-Key' => 'same'), as: :json
    assert_response :created

    post '/api/v1/orders',
         params: order_params(type: 'market_buy', amount: 999),
         headers: bearer(token).merge('Idempotency-Key' => 'same'), as: :json

    assert_response :conflict
    assert_equal 'idempotency_key_reused', JSON.parse(response.body)['error']['code']
  end

  test 'POST /api/v1/orders returns 422 invalid_order_type before consuming the idempotency key' do
    @user.set_rest_tool_enabled('market_buy', true)
    token = api_token

    post '/api/v1/orders',
         params: order_params(type: 'limit_zap'),
         headers: bearer(token).merge('Idempotency-Key' => 'wrong-type'), as: :json

    assert_response :unprocessable_entity
    assert_equal 'invalid_order_type', JSON.parse(response.body)['error']['code']
    # No row should be claimed for an unknown type — the tool gate halts first.
    assert_equal 0, IdempotencyKey.count
  end

  test 'POST /api/v1/orders dispatches limit_buy when type is limit_buy' do
    @user.set_rest_tool_enabled('limit_buy', true)
    Exchanges::Binance.any_instance.expects(:set_client).once.returns(true)
    Exchanges::Binance.any_instance.expects(:limit_buy)
                      .with(has_entries(ticker: @ticker, amount: 100, amount_type: :quote, price: 50_000))
                      .returns(stub(success?: true, data: 'lb_1'))
    token = api_token

    post '/api/v1/orders',
         params: order_params(type: 'limit_buy', price: 50_000),
         headers: bearer(token).merge('Idempotency-Key' => 'lb'), as: :json

    assert_response :created
    body = JSON.parse(response.body)
    assert_equal 'limit', body['data']['order_type']
    assert_equal 'buy', body['data']['side']
  end

  test 'POST /api/v1/orders is 403 tool_disabled for the specific order type even if other types are enabled' do
    @user.set_rest_tool_enabled('market_buy', true)
    # market_sell remains disabled by default — must be rejected.
    token = api_token

    post '/api/v1/orders',
         params: order_params(type: 'market_sell'),
         headers: bearer(token).merge('Idempotency-Key' => 'ms'), as: :json

    assert_response :forbidden
    body = JSON.parse(response.body)
    assert_equal 'tool_disabled', body['error']['code']
    assert_includes body['error']['message'], 'market_sell'
    assert_equal 0, IdempotencyKey.count
  end

  test 'POST /api/v1/orders stores a failed upstream response and replays it without re-hitting exchange' do
    @user.set_rest_tool_enabled('market_buy', true)
    Exchanges::Binance.any_instance.expects(:set_client).once.returns(true)
    Exchanges::Binance.any_instance.expects(:market_buy).once.returns(stub(success?: false, errors: ['rejected by exchange']))
    token = api_token

    post '/api/v1/orders',
         params: order_params(type: 'market_buy'),
         headers: bearer(token).merge('Idempotency-Key' => 'failkey'), as: :json
    first_body = response.body
    assert_response :bad_gateway
    assert_equal 'order_failed', JSON.parse(first_body)['error']['code']

    post '/api/v1/orders',
         params: order_params(type: 'market_buy'),
         headers: bearer(token).merge('Idempotency-Key' => 'failkey'), as: :json

    assert_response :bad_gateway
    assert_equal first_body, response.body
  end

  # ---- DELETE /api/v1/orders/:id ------------------------------------------

  test 'DELETE /api/v1/orders/:id does NOT require Idempotency-Key' do
    @user.set_rest_tool_enabled('cancel_order', true)
    bot = create(:dca_single_asset, user: @user, base_asset: @btc, quote_asset: @usd, exchange: @exchange)
    txn = create(:transaction, bot: bot, status: :submitted, side: :buy, external_status: :open)
    Transaction.any_instance.expects(:cancel).returns(stub(success?: true))
    token = api_token

    # No Idempotency-Key header. A 400 would mean the concern was wired in
    # by mistake — cancellation is deliberately not idempotent-wrapped.
    delete "/api/v1/orders/#{txn.id}", headers: bearer(token)

    assert_response :ok
    assert_equal true, JSON.parse(response.body)['data']['cancelled']
  end

  test 'DELETE /api/v1/orders/:id is 403 tool_disabled by default' do
    bot = create(:dca_single_asset, user: @user, base_asset: @btc, quote_asset: @usd, exchange: @exchange)
    txn = create(:transaction, bot: bot, status: :submitted)
    token = api_token

    delete "/api/v1/orders/#{txn.id}", headers: bearer(token)

    assert_response :forbidden
    assert_equal 'tool_disabled', JSON.parse(response.body)['error']['code']
  end

  test 'DELETE /api/v1/orders/:id returns 422 when cancelling a non-numeric order id without exchange_name' do
    @user.set_rest_tool_enabled('cancel_order', true)
    token = api_token

    delete '/api/v1/orders/EXT-abc', headers: bearer(token)

    assert_response :unprocessable_entity
    assert_equal 'exchange_name_required', JSON.parse(response.body)['error']['code']
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

  def order_params(type:, amount: 100, price: nil)
    base = { type: type, exchange_name: 'Binance', base_asset: 'BTC',
             quote_asset: 'USD', amount: amount }
    base[:price] = price if price
    base
  end

  def stub_exchange_success
    Exchanges::Binance.any_instance.expects(:set_client).at_least_once.returns(true)
    Exchanges::Binance.any_instance.expects(:market_buy).at_least_once.returns(stub(success?: true, data: 'order_abc'))
  end
end
