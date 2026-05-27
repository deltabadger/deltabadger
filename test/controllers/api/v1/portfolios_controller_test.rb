# frozen_string_literal: true

require 'test_helper'

class Api::V1::PortfoliosControllerTest < ActionDispatch::IntegrationTest
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

  test 'GET /api/v1/portfolio returns the empty-portfolio shape when user has no bots' do
    @user.set_rest_tool_enabled('get_portfolio_summary', true)
    token = api_token

    get '/api/v1/portfolio', headers: bearer(token)

    assert_response :ok
    body = JSON.parse(response.body)
    assert_equal true, body['data']['empty']
    assert_equal [], body['data']['bots']
    assert_nil body['data']['totals']
  end

  test 'GET /api/v1/portfolio returns totals + per-bot rows when user has bots' do
    @user.set_rest_tool_enabled('get_portfolio_summary', true)
    create(:dca_single_asset, user: @user, status: :scheduled, started_at: Time.current)
    User.any_instance.stubs(:global_pnl).returns(nil)
    Bots::DcaSingleAsset.any_instance.stubs(:metrics).returns(nil)
    token = api_token

    get '/api/v1/portfolio', headers: bearer(token)

    assert_response :ok
    body = JSON.parse(response.body)
    assert_equal false, body['data']['empty']
    assert_equal 1, body['data']['totals']['total']
    assert_equal 1, body['data']['totals']['working']
    assert_equal 1, body['data']['bots'].size
  end

  test 'GET /api/v1/portfolio is 403 tool_disabled by default' do
    token = api_token
    get '/api/v1/portfolio', headers: bearer(token)
    assert_response :forbidden
    assert_equal 'tool_disabled', JSON.parse(response.body)['error']['code']
  end

  test 'GET /api/v1/portfolio rejects a token without :api scope' do
    @user.set_rest_tool_enabled('get_portfolio_summary', true)
    mcp_only_token = Doorkeeper::AccessToken.create!(
      application: @oauth_app, resource_owner_id: @user.id,
      token: SecureRandom.hex(32), scopes: 'mcp', expires_in: 3600
    )

    get '/api/v1/portfolio', headers: bearer(mcp_only_token)

    assert_response :forbidden
    assert_equal 'insufficient_scope', JSON.parse(response.body)['error']['code']
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
