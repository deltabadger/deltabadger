# frozen_string_literal: true

require 'test_helper'

# Every catalogued tool must gate a routed REST action. The table is the contract: a tool added
# to the catalogue without a row here, or a row whose request is refused for any reason other
# than that tool being off, fails.
class Api::V1::ToolGatingTest < ActionDispatch::IntegrationTest
  GATES = {
    'list_bots' => [:get, '/api/v1/bots'],
    'get_bot_details' => [:get, '/api/v1/bots/1'],
    'create_bot' => [:post, '/api/v1/bots'],
    'update_bot_settings' => [:patch, '/api/v1/bots/1'],
    'start_bot' => [:post, '/api/v1/bots/1/start'],
    'stop_bot' => [:post, '/api/v1/bots/1/stop'],
    'list_exchanges' => [:get, '/api/v1/exchanges'],
    'get_exchange_balances' => [:get, '/api/v1/exchanges/1/balances'],
    'list_transactions' => [:get, '/api/v1/transactions'],
    'list_account_transactions' => [:get, '/api/v1/transactions/account'],
    'export_transactions_csv' => [:get, '/api/v1/transactions/export'],
    'get_portfolio_summary' => [:get, '/api/v1/portfolio'],
    'list_open_orders' => [:get, '/api/v1/orders'],
    'market_buy' => [:post, '/api/v1/orders', { type: 'market_buy' }],
    'market_sell' => [:post, '/api/v1/orders', { type: 'market_sell' }],
    'limit_buy' => [:post, '/api/v1/orders', { type: 'limit_buy' }],
    'limit_sell' => [:post, '/api/v1/orders', { type: 'limit_sell' }],
    'cancel_order' => [:delete, '/api/v1/orders/1'],
    'liquidate_exited_asset' => [:post, '/api/v1/bots/1/liquidations'],
    'answer_redeploy_offer' => [:post, '/api/v1/bots/1/redeploy'],
    'start_rule' => [:post, '/api/v1/rules/1/start'],
    'stop_rule' => [:post, '/api/v1/rules/1/stop'],
    'update_rule_settings' => [:patch, '/api/v1/rules/1'],
    'list_rules' => [:get, '/api/v1/rules'],
    'create_rule' => [:post, '/api/v1/rules'],
    'delete_rule' => [:delete, '/api/v1/rules/1'],
    'list_indices' => [:get, '/api/v1/indices'],
    'create_index_bot' => [:post, '/api/v1/bots', { type: 'index' }],
    'delete_bot' => [:delete, '/api/v1/bots/1'],
    'archive_bot' => [:post, '/api/v1/bots/1/archive'],
    'unarchive_bot' => [:delete, '/api/v1/bots/1/archive'],
    'list_tax_jurisdictions' => [:get, '/api/v1/tax/jurisdictions'],
    'generate_tax_report' => [:post, '/api/v1/tax/reports'],
    'get_tax_report_status' => [:get, '/api/v1/tax/reports/DE/2025'],
    'download_tax_report' => [:get, '/api/v1/tax/reports/DE/2025/download']
  }.freeze

  setup do
    @user = create(:user)
    @oauth_app = Doorkeeper::Application.create!(
      name: 'Gate', redirect_uri: 'http://localhost/callback', confidential: false, scopes: 'api'
    )
    ConnectedClient.create!(user: @user, oauth_application: @oauth_app, rest_tools: AppConfig::REST_TOOL_DEFAULTS.keys)
    @token = Doorkeeper::AccessToken.create!(
      application: @oauth_app, resource_owner_id: @user.id,
      token: SecureRandom.hex(32), scopes: 'api', expires_in: 3600
    )
  end

  teardown do
    Doorkeeper::AccessToken.delete_all
    Doorkeeper::Application.destroy_all
  end

  test 'the table names every catalogued tool, and nothing else' do
    assert_equal AppConfig::REST_TOOL_DEFAULTS.keys.sort, GATES.keys.sort
  end

  GATES.each do |tool, (verb, path, params)|
    test "#{tool} gates #{verb.upcase} #{path}" do
      public_send(verb, path, params: params || {}, headers: { 'Authorization' => "Bearer #{@token.token}" })

      assert_response :forbidden
      body = JSON.parse(response.body)
      assert_equal 'tool_disabled', body['error']['code']
      assert_includes body['error']['message'], "'#{tool}'"
    end
  end
end
