require 'test_helper'

class SettingsMcpTest < ActionDispatch::IntegrationTest
  include Devise::Test::IntegrationHelpers

  setup do
    @admin = create(:user, admin: true, setup_completed: true)
    sign_in @admin
  end

  teardown do
    Doorkeeper::Application.destroy_all
    Doorkeeper::AccessToken.delete_all
  end

  test 'mcp widget shows URL' do
    get settings_connect_path
    assert_response :success
    assert_select '#mcp_url_display'
  end

  test 'mcp widget shows connected clients section' do
    get settings_connect_path
    assert_response :success
    assert_select '#mcp_connected_clients'
  end

  test 'mcp widget shows client when one exists' do
    app = Doorkeeper::Application.create!(name: 'Test Client', redirect_uri: 'http://localhost/callback', confidential: false)
    Doorkeeper::AccessToken.create!(application: app, resource_owner_id: @admin.id, token: SecureRandom.hex(32), expires_in: 3600)
    ConnectedClient.create!(user: @admin, oauth_application: app, mcp_tools: AppConfig::MCP_TOOL_GROUPS['read'])

    get settings_connect_path
    assert_response :success
    assert_select '#mcp_connected_clients', /Test Client/
  end

  test 'revoke client removes application and tokens' do
    app = Doorkeeper::Application.create!(name: 'Test Client', redirect_uri: 'http://localhost/callback', confidential: false)
    Doorkeeper::AccessToken.create!(application: app, resource_owner_id: @admin.id, token: SecureRandom.hex(32), expires_in: 3600)

    # Capture first: destroying the application cascades its tokens away, so
    # asserting `.all?(&:revoked?)` over the relation afterwards would be asserting
    # nothing at all — [].all? is true.
    token_ids = Doorkeeper::AccessToken.where(application_id: app.id).pluck(:id)
    assert_not_empty token_ids

    assert_difference 'Doorkeeper::Application.count', -1 do
      delete settings_revoke_mcp_client_path(id: app.id)
    end

    assert_response :success
    assert_empty Doorkeeper::AccessToken.where(id: token_ids),
                 'destroying the application cascades its tokens away'
  end

  test 'user cannot revoke another users client' do
    regular_user = create(:user, setup_completed: true)
    sign_in regular_user
    app = Doorkeeper::Application.create!(name: 'Test Client', redirect_uri: 'http://localhost/callback', confidential: false)
    Doorkeeper::AccessToken.create!(application: app, resource_owner_id: @admin.id, token: SecureRandom.hex(32), expires_in: 3600)

    assert_no_difference 'Doorkeeper::Application.count' do
      delete settings_revoke_mcp_client_path(id: app.id)
    end
    assert_response :not_found
  end

  test 'mcp widget is shown to non-admin users' do
    regular_user = create(:user, setup_completed: true)
    sign_in regular_user

    get settings_connect_path
    assert_response :success
    assert_select 'turbo-frame#mcp_settings'
  end
end
