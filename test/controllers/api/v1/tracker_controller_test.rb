# frozen_string_literal: true

require 'test_helper'

class Api::V1::TrackerControllerTest < ActionDispatch::IntegrationTest
  setup do
    @user = create(:user)
    @oauth_app = Doorkeeper::Application.create!(
      name: 'Test', redirect_uri: 'http://localhost/callback', confidential: false, scopes: 'api'
    )
    ConnectedClient.create!(user: @user, oauth_application: @oauth_app, rest_tools: AppConfig::REST_TOOL_DEFAULTS.keys)
  end

  teardown do
    Doorkeeper::AccessToken.delete_all
    Doorkeeper::Application.destroy_all
  end

  test 'POST /tracker/sync queues both syncs and returns 202' do
    @user.set_rest_tool_enabled('sync_tracker', true)
    api_key = create(:api_key, user: @user)
    AccountTransaction::SyncTrackerJob.expects(:perform_later).with(@user.id, [api_key.id])
    AccountBalance::SyncJob.expects(:perform_later).with(@user.id, [api_key.id])

    post '/api/v1/tracker/sync', headers: bearer(api_token)

    assert_response :accepted
    assert_equal ['Binance'], JSON.parse(response.body)['data']['exchanges']
  end

  test 'POST /tracker/sync with nothing to sync is a 422' do
    @user.set_rest_tool_enabled('sync_tracker', true)
    AccountTransaction::SyncTrackerJob.expects(:perform_later).never

    post '/api/v1/tracker/sync', headers: bearer(api_token)

    assert_response :unprocessable_entity
    assert_equal 'no_reading_keys', JSON.parse(response.body)['error']['code']
  end

  test 'POST /tracker/sync is gated' do
    post '/api/v1/tracker/sync', headers: bearer(api_token)
    assert_response :forbidden
  end

  private

  def bearer(token) = { 'Authorization' => "Bearer #{token.token}" }

  def api_token
    @api_token ||= Doorkeeper::AccessToken.create!(
      application: @oauth_app, resource_owner_id: @user.id,
      token: SecureRandom.hex(32), scopes: 'api', expires_in: 3600
    )
  end
end
