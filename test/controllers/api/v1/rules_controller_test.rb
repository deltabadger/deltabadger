# frozen_string_literal: true

require 'test_helper'

class Api::V1::RulesControllerTest < ActionDispatch::IntegrationTest
  setup do
    @user = create(:user)
    @exchange = create(:binance_exchange)
    @asset = create(:asset, :bitcoin)
    @rule = Rules::Withdrawal.create!(
      user: @user, exchange: @exchange, asset: @asset,
      address: '0xabc123', status: :stopped,
      settings: { 'max_fee_percentage' => '5', 'threshold_type' => 'fee_percentage' }
    )

    @oauth_app = Doorkeeper::Application.create!(
      name: 'Test', redirect_uri: 'http://localhost/callback',
      confidential: false, scopes: 'api'
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

  # ---- start --------------------------------------------------------------

  test 'POST /api/v1/rules/:id/start starts a stopped rule when start_rule is enabled' do
    @user.set_rest_tool_enabled('start_rule', true)
    token = api_token

    post "/api/v1/rules/#{@rule.id}/start", headers: bearer(token)

    assert_response :ok
    body = JSON.parse(response.body)
    assert_equal @rule.id, body['data']['id']
    @rule.reload
    assert @rule.working?
  end

  test 'POST /api/v1/rules/:id/start returns 404 rule_not_found for unknown id' do
    @user.set_rest_tool_enabled('start_rule', true)
    token = api_token

    post '/api/v1/rules/999999/start', headers: bearer(token)

    assert_response :not_found
    assert_equal 'rule_not_found', JSON.parse(response.body)['error']['code']
  end

  test 'POST /api/v1/rules/:id/start returns 409 when rule is already active' do
    @user.set_rest_tool_enabled('start_rule', true)
    @rule.update!(status: :scheduled)
    token = api_token

    post "/api/v1/rules/#{@rule.id}/start", headers: bearer(token)

    assert_response :conflict
    assert_equal 'rule_already_active', JSON.parse(response.body)['error']['code']
  end

  test 'POST /api/v1/rules/:id/start is 403 tool_disabled by default' do
    token = api_token
    post "/api/v1/rules/#{@rule.id}/start", headers: bearer(token)
    assert_response :forbidden
    assert_equal 'tool_disabled', JSON.parse(response.body)['error']['code']
  end

  # ---- stop ---------------------------------------------------------------

  test 'POST /api/v1/rules/:id/stop stops an active rule when stop_rule is enabled' do
    @user.set_rest_tool_enabled('stop_rule', true)
    @rule.update!(status: :scheduled)
    token = api_token

    post "/api/v1/rules/#{@rule.id}/stop", headers: bearer(token)

    assert_response :ok
    @rule.reload
    assert_not @rule.working?
  end

  test 'POST /api/v1/rules/:id/stop returns 409 when rule is not active' do
    @user.set_rest_tool_enabled('stop_rule', true)
    token = api_token

    post "/api/v1/rules/#{@rule.id}/stop", headers: bearer(token)

    assert_response :conflict
    assert_equal 'rule_not_active', JSON.parse(response.body)['error']['code']
  end

  # ---- update -------------------------------------------------------------

  test 'PATCH /api/v1/rules/:id updates the supplied fields on a stopped rule' do
    @user.set_rest_tool_enabled('update_rule_settings', true)
    token = api_token

    # Switching the threshold also supplies the value it will read, or the rule would save in a
    # state that only fails at its next start.
    patch "/api/v1/rules/#{@rule.id}",
          params: { max_fee_percentage: 2.5, threshold_type: 'min_amount', min_amount: 0.25 },
          headers: bearer(token), as: :json

    assert_response :ok
    body = JSON.parse(response.body)
    assert_equal @rule.id, body['data']['id']
    assert_equal %w[max_fee_percentage min_amount threshold_type].sort, body['data']['updated'].sort
    @rule.reload
    assert_equal '2.5', @rule.settings['max_fee_percentage']
    assert_equal 'min_amount', @rule.settings['threshold_type']
    assert_equal '0.25', @rule.settings['min_amount']
  end

  test 'PATCH /api/v1/rules/:id with no updatable params returns 422' do
    @user.set_rest_tool_enabled('update_rule_settings', true)
    token = api_token

    patch "/api/v1/rules/#{@rule.id}", params: {}, headers: bearer(token), as: :json

    assert_response :unprocessable_entity
    assert_equal 'no_updates_provided', JSON.parse(response.body)['error']['code']
  end

  test 'PATCH /api/v1/rules/:id returns 409 when rule is running' do
    @user.set_rest_tool_enabled('update_rule_settings', true)
    @rule.update!(status: :scheduled)
    token = api_token

    patch "/api/v1/rules/#{@rule.id}",
          params: { max_fee_percentage: 2.5 },
          headers: bearer(token), as: :json

    assert_response :conflict
    assert_equal 'rule_active', JSON.parse(response.body)['error']['code']
  end

  test 'PATCH /api/v1/rules/:id is 403 tool_disabled by default' do
    token = api_token
    patch "/api/v1/rules/#{@rule.id}",
          params: { max_fee_percentage: 2.5 },
          headers: bearer(token), as: :json
    assert_response :forbidden
    assert_equal 'tool_disabled', JSON.parse(response.body)['error']['code']
  end

  # ---- list / create / delete ---------------------------------------------

  test 'GET /api/v1/rules lists the rules with masked destinations' do
    @user.set_rest_tool_enabled('list_rules', true)

    get '/api/v1/rules', headers: bearer(api_token)

    assert_response :ok
    body = JSON.parse(response.body)['data']
    assert_equal 1, body['count']
    # Short enough that only a prefix survives; never the whole destination.
    assert_equal '0x…', body['rules'].first['address']
  end

  test 'POST /api/v1/rules creates a stopped rule' do
    @user.set_rest_tool_enabled('create_rule', true)
    cold = with_allow_listed_address

    post '/api/v1/rules',
         params: { exchange_name: 'Binance', asset: 'ETH', address: cold },
         headers: bearer(api_token), as: :json

    assert_response :created
    rule = @user.rules.find(JSON.parse(response.body)['data']['id'])
    assert rule.created?
    assert_equal cold, rule.address
  end

  test 'POST /api/v1/rules refuses an unlisted address without naming the allow-list' do
    @user.set_rest_tool_enabled('create_rule', true)
    cold = with_allow_listed_address

    post '/api/v1/rules',
         params: { exchange_name: 'Binance', asset: 'ETH', address: 'bc1qstranger' },
         headers: bearer(api_token), as: :json

    assert_response :unprocessable_entity
    body = JSON.parse(response.body)
    assert_equal 'address_not_listed', body['error']['code']
    assert_nil body['data']
    assert_not_includes body['error']['message'], cold
  end

  test 'DELETE /api/v1/rules/:id deletes a stopped rule' do
    @user.set_rest_tool_enabled('delete_rule', true)

    delete "/api/v1/rules/#{@rule.id}", headers: bearer(api_token)

    assert_response :ok
    assert @rule.reload.deleted?
  end

  test 'the three new actions are gated by their own tools' do
    token = api_token
    get '/api/v1/rules', headers: bearer(token)
    assert_response :forbidden
    post '/api/v1/rules', headers: bearer(token)
    assert_response :forbidden
    delete "/api/v1/rules/#{@rule.id}", headers: bearer(token)
    assert_response :forbidden
  end

  private

  # A second asset, so the fixture rule's BTC slot stays free for the uniqueness check.
  def with_allow_listed_address
    eth = create(:asset, :ethereum)
    create(:ticker, exchange: @exchange, base_asset: eth, quote_asset: create(:asset, :usd))
    create(:api_key, user: @user, exchange: @exchange, key_type: :withdrawal, status: :correct)
    cold = 'bc1qxy2kgdygjrsqtzq2n0yrf2493p83kkfjhx0wlh'
    Exchanges::Binance.any_instance.stubs(:set_client)
    Exchanges::Binance.any_instance.stubs(:withdrawal_fee_fresh?).returns(true)
    Exchanges::Binance.any_instance.stubs(:list_withdrawal_addresses).returns([{ name: cold }])
    Rules::Withdrawal.any_instance.stubs(:withdrawal_fee_known?).returns(true)
    cold
  end

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
