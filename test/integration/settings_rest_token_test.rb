# frozen_string_literal: true

require 'test_helper'

class SettingsRestTokenTest < ActionDispatch::IntegrationTest
  include Devise::Test::IntegrationHelpers

  setup do
    @user = create(:user, admin: true, setup_completed: true)
    sign_in @user
  end

  teardown do
    Doorkeeper::AccessToken.delete_all
    Doorkeeper::Application.delete_all
  end

  # ---- widget renders the token (lazy mint) --------------------------------

  test 'GET /settings/connect creates the personal token lazily on first visit and renders it' do
    # Pre-condition: no personal app exists yet.
    assert_equal 0, Doorkeeper::Application.where(personal_owner_id: @user.id,
                                                  personal_access_token: true).count

    get settings_connect_path
    assert_response :success

    # Post-condition: exactly one personal app + token, both surfaced in the page.
    @user.reload
    token = @user.personal_api_application&.access_tokens&.where(revoked_at: nil)&.first
    assert token, 'personal token was not created on first visit'
    assert_includes response.body, token.token
  end

  test 'GET /settings/connect on a second visit reuses the same token (no churn)' do
    get settings_connect_path
    first = @user.reload.personal_api_application.access_tokens.where(revoked_at: nil).first

    get settings_connect_path
    second = @user.reload.personal_api_application.access_tokens.where(revoked_at: nil).first

    assert_equal first.id, second.id
    assert_equal first.token, second.token
  end

  # ---- regenerate end-to-end ----------------------------------------------

  test 'POST /settings/regenerate_api_token revokes old token and issues a new one (verified via /api/v1/bots)' do
    # 1. Surface the token + enable list_bots so REST is reachable.
    get settings_connect_path
    @user.set_rest_tool_enabled('list_bots', true)
    old_token_str = @user.reload.personal_api_token.token

    # 2. Old token works against REST.
    get '/api/v1/bots', headers: { 'Authorization' => "Bearer #{old_token_str}" }
    assert_response :ok

    # 3. Regenerate.
    post settings_regenerate_api_token_path
    assert_response :success
    new_token_str = @user.reload.personal_api_token.token
    assert_not_equal old_token_str, new_token_str

    # 4. Old token is rejected.
    get '/api/v1/bots', headers: { 'Authorization' => "Bearer #{old_token_str}" }
    assert_response :unauthorized
    assert_equal 'token_revoked', JSON.parse(response.body)['error']['code']

    # 5. New token works.
    get '/api/v1/bots', headers: { 'Authorization' => "Bearer #{new_token_str}" }
    assert_response :ok
  end

  test 'POST /settings/regenerate_api_token re-renders the REST widget with the new token visible' do
    get settings_connect_path
    old_token = @user.reload.personal_api_token.token

    post settings_regenerate_api_token_path
    assert_response :success

    new_token = @user.reload.personal_api_token.token
    assert_not_equal old_token, new_token
    # The turbo_stream response replaces #rest_settings — assert the new token
    # string is in the rendered fragment.
    assert_includes response.body, new_token
    assert_not_includes response.body, old_token
  end

  # ---- isolation from MCP widget ------------------------------------------

  test 'MCP Connected clients section does NOT list the personal application' do
    get settings_connect_path # triggers lazy mint
    @user.reload

    get settings_connect_path
    assert_response :success
    # The personal app's name appears nowhere in the MCP Connected clients section
    # (which has the `#mcp_connected_clients` DOM id from the MCP widget).
    assert_select '#mcp_connected_clients' do |frame|
      assert_no_match(/Personal API token/, frame.to_s)
    end
  end

  # ---- authorization ------------------------------------------------------

  test 'POST /settings/regenerate_api_token without a signed-in session is rejected' do
    sign_out @user
    post settings_regenerate_api_token_path

    # Devise redirects unauthenticated requests to login.
    assert_response :redirect
  end

  test 'regenerate scopes by current_user — user A cannot affect user B token' do
    other = create(:user, setup_completed: true)
    sign_in other
    get settings_connect_path # triggers lazy mint for `other`
    other_token = other.reload.personal_api_token.token

    sign_in @user
    get settings_connect_path
    my_token_before = @user.reload.personal_api_token.token

    post settings_regenerate_api_token_path
    assert_response :success

    # My token changed; other user's didn't.
    assert_not_equal my_token_before, @user.reload.personal_api_token.token
    assert_equal other_token, other.reload.personal_api_token.token
  end
end
