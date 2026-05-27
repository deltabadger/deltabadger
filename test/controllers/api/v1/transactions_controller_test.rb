# frozen_string_literal: true

require 'test_helper'

class Api::V1::TransactionsControllerTest < ActionDispatch::IntegrationTest
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

  # ---- /api/v1/transactions ------------------------------------------------

  test 'GET /api/v1/transactions returns transactions when list_transactions is enabled' do
    @user.set_rest_tool_enabled('list_transactions', true)
    bot = create(:dca_single_asset, user: @user)
    create(:transaction, bot: bot, status: :submitted,
                         amount_exec: 0.001, price: 50_000, quote_amount_exec: 50)
    token = api_token

    get '/api/v1/transactions', headers: bearer(token)

    assert_response :ok
    body = JSON.parse(response.body)
    assert_equal 1, body['data']['count']
    row = body['data']['transactions'].first
    assert_equal 'buy', row['side']
    assert_equal bot.id, row['bot_id']
  end

  test 'GET /api/v1/transactions returns 403 tool_disabled by default' do
    token = api_token
    get '/api/v1/transactions', headers: bearer(token)
    assert_response :forbidden
    assert_equal 'tool_disabled', JSON.parse(response.body)['error']['code']
  end

  test 'GET /api/v1/transactions respects bot_id and limit params' do
    @user.set_rest_tool_enabled('list_transactions', true)
    btc = create(:asset, :bitcoin)
    eth = create(:asset, :ethereum)
    usd = create(:asset, :usd)
    exchange = create(:binance_exchange)
    bot1 = create(:dca_single_asset, user: @user, base_asset: btc, quote_asset: usd, exchange: exchange)
    bot2 = create(:dca_single_asset, user: @user, base_asset: eth, quote_asset: usd, exchange: exchange)
    2.times { create(:transaction, bot: bot1) }
    create(:transaction, bot: bot2)
    token = api_token

    get '/api/v1/transactions', params: { bot_id: bot1.id, limit: 1 }, headers: bearer(token)

    assert_response :ok
    body = JSON.parse(response.body)
    assert_equal 1, body['data']['count']
    assert_equal bot1.id, body['data']['transactions'].first['bot_id']
  end

  test 'GET /api/v1/transactions returns 404 bot_not_found when bot_id is unknown' do
    @user.set_rest_tool_enabled('list_transactions', true)
    token = api_token

    get '/api/v1/transactions', params: { bot_id: 999_999 }, headers: bearer(token)

    assert_response :not_found
    assert_equal 'bot_not_found', JSON.parse(response.body)['error']['code']
  end

  # ---- /api/v1/transactions/account ----------------------------------------

  test 'GET /api/v1/transactions/account returns rows when list_account_transactions is enabled' do
    @user.set_rest_tool_enabled('list_account_transactions', true)
    exchange = create(:binance_exchange)
    api_key = create(:api_key, user: @user, exchange: exchange, key_type: :trading)
    create(:account_transaction, api_key: api_key, entry_type: :buy, transacted_at: 1.day.ago)
    token = api_token

    get '/api/v1/transactions/account', headers: bearer(token)

    assert_response :ok
    body = JSON.parse(response.body)
    assert_equal 1, body['data']['count']
    assert_equal 'buy', body['data']['transactions'].first['entry_type']
  end

  test 'GET /api/v1/transactions/account returns 404 exchange_not_found for unknown exchange_id' do
    @user.set_rest_tool_enabled('list_account_transactions', true)
    token = api_token

    get '/api/v1/transactions/account', params: { exchange_id: 999_999 }, headers: bearer(token)

    assert_response :not_found
    assert_equal 'exchange_not_found', JSON.parse(response.body)['error']['code']
  end

  test 'GET /api/v1/transactions/account returns 422 for an unparseable date' do
    @user.set_rest_tool_enabled('list_account_transactions', true)
    token = api_token

    get '/api/v1/transactions/account', params: { from_date: 'not-a-date' }, headers: bearer(token)

    assert_response :unprocessable_entity
    assert_equal 'invalid_date', JSON.parse(response.body)['error']['code']
  end

  test 'GET /api/v1/transactions/account is 403 tool_disabled by default' do
    token = api_token
    get '/api/v1/transactions/account', headers: bearer(token)
    assert_response :forbidden
    assert_equal 'tool_disabled', JSON.parse(response.body)['error']['code']
  end

  # ---- /api/v1/transactions/export (CSV) ----------------------------------
  # This is the only REST endpoint that does NOT use the JSON envelope on
  # success — it serves `text/csv` directly. Error responses still use the
  # envelope so clients can parse them uniformly.

  test 'GET /api/v1/transactions/export returns text/csv with attachment header' do
    @user.set_rest_tool_enabled('export_transactions_csv', true)
    exchange = create(:binance_exchange)
    api_key = create(:api_key, user: @user, exchange: exchange, key_type: :trading)
    create(:account_transaction, api_key: api_key, entry_type: :buy, base_currency: 'BTC',
                                 base_amount: 0.5, transacted_at: 1.day.ago)
    token = api_token

    get '/api/v1/transactions/export', headers: bearer(token)

    assert_response :ok
    assert_match(%r{text/csv}, response.headers['Content-Type'])
    assert_match(/attachment; filename="account_transactions_/, response.headers['Content-Disposition'])
    # Body is raw CSV — not JSON-wrapped.
    assert_no_match(/^\{"data":/, response.body)
    assert_match(/BTC/, response.body)
  end

  test 'GET /api/v1/transactions/export surfaces total/returned/truncated as headers' do
    @user.set_rest_tool_enabled('export_transactions_csv', true)
    exchange = create(:binance_exchange)
    api_key = create(:api_key, user: @user, exchange: exchange, key_type: :trading)
    create(:account_transaction, api_key: api_key, entry_type: :buy, transacted_at: 1.day.ago)
    token = api_token

    get '/api/v1/transactions/export', headers: bearer(token)

    assert_equal '1', response.headers['X-Total-Transactions']
    assert_equal '1', response.headers['X-Returned-Transactions']
    assert_equal 'false', response.headers['X-Truncated']
  end

  test 'GET /api/v1/transactions/export returns JSON-envelope 404 when no rows match' do
    @user.set_rest_tool_enabled('export_transactions_csv', true)
    token = api_token

    get '/api/v1/transactions/export', headers: bearer(token)

    assert_response :not_found
    assert_match(%r{application/json}, response.headers['Content-Type'])
    body = JSON.parse(response.body)
    assert_equal 'no_transactions', body['error']['code']
  end

  test 'GET /api/v1/transactions/export returns JSON-envelope 404 for unknown exchange_id' do
    @user.set_rest_tool_enabled('export_transactions_csv', true)
    token = api_token

    get '/api/v1/transactions/export', params: { exchange_id: 999_999 }, headers: bearer(token)

    assert_response :not_found
    assert_equal 'exchange_not_found', JSON.parse(response.body)['error']['code']
  end

  test 'GET /api/v1/transactions/export returns JSON-envelope 422 for invalid date' do
    @user.set_rest_tool_enabled('export_transactions_csv', true)
    token = api_token

    get '/api/v1/transactions/export', params: { from_date: 'not-a-date' }, headers: bearer(token)

    assert_response :unprocessable_entity
    assert_equal 'invalid_date', JSON.parse(response.body)['error']['code']
  end

  test 'GET /api/v1/transactions/export is 403 tool_disabled by default' do
    token = api_token
    get '/api/v1/transactions/export', headers: bearer(token)
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
