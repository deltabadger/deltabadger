# frozen_string_literal: true

require 'test_helper'

class Api::V1::TransactionsControllerTest < ActionDispatch::IntegrationTest
  setup do
    @user = create(:user)
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

  # ---- tracker writes ------------------------------------------------------

  def transfer_pair
    api_key = create(:api_key, user: @user)
    at = Time.zone.parse('2026-08-01 12:00:00')
    withdrawal = create(:account_transaction, :withdrawal, api_key: api_key, base_amount: 1, transacted_at: at)
    deposit = create(:account_transaction, :deposit, api_key: api_key, base_amount: 0.999, transacted_at: at + 2.days)
    Tracker::LedgerJob.stubs(:perform_later)
    PortfolioSnapshot::BackfillJob.stubs(:perform_later)
    [withdrawal, deposit]
  end

  test 'POST /transactions/account/:id/transfer_link links the pair' do
    @user.set_rest_tool_enabled('set_transfer_link', true)
    withdrawal, deposit = transfer_pair

    post "/api/v1/transactions/account/#{withdrawal.id}/transfer_link",
         params: { linked: true }, headers: bearer(api_token), as: :json

    assert_response :ok
    assert_equal deposit.id, JSON.parse(response.body)['data']['deposit_id']
    assert_equal deposit.id, withdrawal.reload.linked_transaction_id
  end

  test 'POST /transactions/account/:id/transfer_link needs a real boolean' do
    @user.set_rest_tool_enabled('set_transfer_link', true)
    withdrawal, = transfer_pair

    post "/api/v1/transactions/account/#{withdrawal.id}/transfer_link",
         params: {}, headers: bearer(api_token), as: :json

    assert_response :unprocessable_entity
    assert_equal 'linked_required', JSON.parse(response.body)['error']['code']
    assert_nil withdrawal.reload.linked_transaction_id
  end

  test 'PATCH /transactions/account/:id/price states and clears a price' do
    @user.set_rest_tool_enabled('set_transaction_price', true)
    _, deposit = transfer_pair

    patch "/api/v1/transactions/account/#{deposit.id}/price",
          params: { price_usd: '123.45' }, headers: bearer(api_token), as: :json
    assert_response :ok
    assert_equal BigDecimal('123.45'), deposit.reload.manual_value(:price)

    patch "/api/v1/transactions/account/#{deposit.id}/price",
          params: { price_usd: nil }, headers: bearer(api_token), as: :json
    assert_response :ok
    assert_nil deposit.reload.manual_value(:price)
  end

  test 'PATCH /transactions/account/:id/price refuses a body that omits the key' do
    @user.set_rest_tool_enabled('set_transaction_price', true)
    _, deposit = transfer_pair
    deposit.set_manual(:price, '50')
    deposit.save!

    patch "/api/v1/transactions/account/#{deposit.id}/price", params: {}, headers: bearer(api_token), as: :json

    assert_response :unprocessable_entity
    assert_equal 'price_usd_required', JSON.parse(response.body)['error']['code']
    assert_equal BigDecimal('50'), deposit.reload.manual_value(:price)
  end

  test 'GET /transactions/account reports link state and stated price' do
    @user.set_rest_tool_enabled('list_account_transactions', true)
    withdrawal, deposit = transfer_pair
    withdrawal.update!(linked_transaction: deposit)
    withdrawal.set_manual(:price, '123.45')
    withdrawal.save!

    get '/api/v1/transactions/account', headers: bearer(api_token)

    assert_response :ok
    row = JSON.parse(response.body)['data']['transactions'].find { |r| r['id'] == withdrawal.id }
    assert_equal true, row['linked']
    assert_equal deposit.id, row['linked_transaction_id']
    assert_equal '123.45', row['stated_price_usd']
    assert_equal false, row['venue_valued']
  end

  test 'the two write endpoints are gated by their own tools' do
    withdrawal, = transfer_pair
    token = api_token

    post "/api/v1/transactions/account/#{withdrawal.id}/transfer_link",
         params: { linked: true }, headers: bearer(token), as: :json
    assert_response :forbidden

    patch "/api/v1/transactions/account/#{withdrawal.id}/price",
          params: { price_usd: '1' }, headers: bearer(token), as: :json
    assert_response :forbidden
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
