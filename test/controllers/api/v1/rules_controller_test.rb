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
      settings: { 'max_fee_percentage' => '5', 'threshold_type' => 'max_fee_percentage' }
    )

    @oauth_app = Doorkeeper::Application.create!(
      name: 'Test', redirect_uri: 'http://localhost/callback',
      confidential: false, scopes: 'api'
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

    patch "/api/v1/rules/#{@rule.id}",
          params: { max_fee_percentage: 2.5, threshold_type: 'min_amount' },
          headers: bearer(token), as: :json

    assert_response :ok
    body = JSON.parse(response.body)
    assert_equal @rule.id, body['data']['id']
    assert_equal %w[max_fee_percentage threshold_type].sort, body['data']['updated'].sort
    @rule.reload
    assert_equal '2.5', @rule.settings['max_fee_percentage']
    assert_equal 'min_amount', @rule.settings['threshold_type']
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
