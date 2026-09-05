# frozen_string_literal: true

require 'test_helper'

# POST /bots/:id/liquidations and POST /bots/:id/redeploy can place market orders, so they carry
# the same Idempotency-Key contract as POST /orders — with the key scoped to the bot and the
# action, which the shared concern's body-only fingerprint cannot do on its own.
class Api::V1::BotTradesControllerTest < ActionDispatch::IntegrationTest
  setup do
    @user = create(:user)
    @bot = create(:dca_index, user: @user, status: :scheduled, started_at: Time.current, with_api_key: true)
    Bots::DcaIndex.any_instance.stubs(:exited_symbols).returns(%w[DOGE])
    Bots::DcaIndex.any_instance.stubs(:redeploy_offer).returns(25.to_d)
    Bots::DcaIndex.any_instance.stubs(:ensure_exchange_authenticated)
    Bots::DcaIndex.any_instance.stubs(:composition_tickers).returns([])
    Exchanges::Kraken.any_instance.stubs(:market_open?).returns(true)
    @oauth_app = Doorkeeper::Application.create!(
      name: 'Test', redirect_uri: 'http://localhost/callback', confidential: false, scopes: 'api'
    )
    ConnectedClient.create!(user: @user, oauth_application: @oauth_app, rest_tools: AppConfig::REST_TOOL_DEFAULTS.keys)
    @user.set_rest_tool_enabled('liquidate_exited_asset', true)
    @user.set_rest_tool_enabled('answer_redeploy_offer', true)
  end

  teardown do
    IdempotencyKey.delete_all
    Doorkeeper::AccessToken.delete_all
    Doorkeeper::Application.destroy_all
  end

  test 'POST /bots/:id/liquidations queues the sale and returns 202' do
    Bot::LiquidateExitedJob.expects(:perform_later).with(@bot, symbol: 'DOGE')

    post "/api/v1/bots/#{@bot.id}/liquidations",
         params: { symbol: 'DOGE' }, headers: keyed('k1'), as: :json

    assert_response :accepted
    assert_equal 'DOGE', JSON.parse(response.body)['data']['symbol']
  end

  test 'POST /bots/:id/liquidations without a key is a 400 before anything is queued' do
    Bot::LiquidateExitedJob.expects(:perform_later).never

    post "/api/v1/bots/#{@bot.id}/liquidations", params: { symbol: 'DOGE' }, headers: bearer(api_token), as: :json

    assert_response :bad_request
    assert_equal 'idempotency_key_required', JSON.parse(response.body)['error']['code']
    assert_equal 0, IdempotencyKey.count
  end

  test 'a disabled tool refuses before the key is claimed' do
    @user.set_rest_tool_enabled('liquidate_exited_asset', false)

    post "/api/v1/bots/#{@bot.id}/liquidations", params: { symbol: 'DOGE' }, headers: keyed('k1'), as: :json

    assert_response :forbidden
    assert_equal 'tool_disabled', JSON.parse(response.body)['error']['code']
    assert_equal 0, IdempotencyKey.count
  end

  test 'the same key replayed against the same bot returns the stored response and queues once' do
    Bot::LiquidateExitedJob.expects(:perform_later).with(@bot, symbol: 'DOGE').once

    post "/api/v1/bots/#{@bot.id}/liquidations", params: { symbol: 'DOGE' }, headers: keyed('k1'), as: :json
    first = response.body
    post "/api/v1/bots/#{@bot.id}/liquidations", params: { symbol: 'DOGE' }, headers: keyed('k1'), as: :json

    assert_response :accepted
    assert_equal first, response.body
  end

  # The concern fingerprints the body only, so without the controller's override this identical
  # body against another bot would replay the first bot's 202 and never act on the second.
  test 'the same key and body against another bot is a reuse, not a replay' do
    # Sharing the exchange and quote asset: a second bare :dca_index would re-create Kraken and
    # EUR, and both are uniqueness-validated.
    other = create(:dca_index, user: @user, exchange: @bot.exchange, quote_asset: @bot.quote_asset,
                               status: :scheduled, started_at: Time.current)
    Bot::LiquidateExitedJob.stubs(:perform_later)

    post "/api/v1/bots/#{@bot.id}/liquidations", params: { symbol: 'DOGE' }, headers: keyed('k1'), as: :json
    assert_response :accepted
    post "/api/v1/bots/#{other.id}/liquidations", params: { symbol: 'DOGE' }, headers: keyed('k1'), as: :json

    assert_response :conflict
    assert_equal 'idempotency_key_reused', JSON.parse(response.body)['error']['code']
  end

  # Rails merges the query string into params, so a symbol sent there is what the action acts on;
  # a body-only fingerprint would replay the first answer for a different symbol.
  test 'the same key with a different query parameter is a reuse, not a replay' do
    Bot::LiquidateExitedJob.stubs(:perform_later)
    Bots::DcaIndex.any_instance.stubs(:exited_symbols).returns(%w[DOGE SHIB])

    post "/api/v1/bots/#{@bot.id}/liquidations?symbol=DOGE", headers: keyed('k1'), as: :json
    assert_response :accepted
    post "/api/v1/bots/#{@bot.id}/liquidations?symbol=SHIB", headers: keyed('k1'), as: :json

    assert_response :conflict
    assert_equal 'idempotency_key_reused', JSON.parse(response.body)['error']['code']
  end

  test 'the same key across the two actions is a reuse too' do
    Bot::RedeployJob.stubs(:perform_later)
    Bot::LiquidateExitedJob.stubs(:perform_later)

    post "/api/v1/bots/#{@bot.id}/redeploy", params: { accept: true }, headers: keyed('k1'), as: :json
    assert_response :accepted
    post "/api/v1/bots/#{@bot.id}/liquidations", params: { accept: true }, headers: keyed('k1'), as: :json

    assert_response :conflict
    assert_equal 'idempotency_key_reused', JSON.parse(response.body)['error']['code']
  end

  test 'POST /bots/:id/redeploy reports the offer it queued' do
    Bot::RedeployJob.expects(:perform_later).with(@bot)

    post "/api/v1/bots/#{@bot.id}/redeploy", params: { accept: true }, headers: keyed('k1'), as: :json

    assert_response :accepted
    body = JSON.parse(response.body)['data']
    assert_equal true, body['accepted']
    assert_equal '25.0', body['offer']
  end

  test 'a non-boolean accept is a 422, never a decline' do
    Bot::DeclineRedeployJob.expects(:perform_later).never
    Bot::RedeployJob.expects(:perform_later).never

    post "/api/v1/bots/#{@bot.id}/redeploy", params: { accept: 'maybe' }, headers: keyed('k1'), as: :json

    assert_response :unprocessable_entity
    assert_equal 'accept_required', JSON.parse(response.body)['error']['code']
  end

  test 'a closed market is a 409 on both endpoints' do
    Exchanges::Kraken.any_instance.stubs(:market_open?).returns(false)

    post "/api/v1/bots/#{@bot.id}/liquidations", params: { symbol: 'DOGE' }, headers: keyed('k1'), as: :json
    assert_response :conflict
    assert_equal 'market_closed', JSON.parse(response.body)['error']['code']

    post "/api/v1/bots/#{@bot.id}/redeploy", params: { accept: true }, headers: keyed('k2'), as: :json
    assert_response :conflict
  end

  test 'the redeploy endpoint is gated by its own tool' do
    @user.set_rest_tool_enabled('answer_redeploy_offer', false)

    post "/api/v1/bots/#{@bot.id}/redeploy", params: { accept: true }, headers: keyed('k1'), as: :json

    assert_response :forbidden
  end

  private

  def bearer(token) = { 'Authorization' => "Bearer #{token.token}" }

  def keyed(key) = bearer(api_token).merge('Idempotency-Key' => key)

  def api_token
    @api_token ||= Doorkeeper::AccessToken.create!(
      application: @oauth_app, resource_owner_id: @user.id,
      token: SecureRandom.hex(32), scopes: 'api', expires_in: 3600
    )
  end
end
