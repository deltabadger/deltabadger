require 'test_helper'

class SettingsMcpTest < ActionDispatch::IntegrationTest
  include Devise::Test::IntegrationHelpers

  setup do
    @admin = create(:user, admin: true, setup_completed: true)
    sign_in @admin
    @original_mcp_enabled = ENV['MCP_ENABLED']
    ENV['MCP_ENABLED'] = 'true'
  end

  teardown do
    AppConfig.clear_mcp_settings!
    Doorkeeper::Application.destroy_all
    Doorkeeper::AccessToken.delete_all
    ENV['MCP_ENABLED'] = @original_mcp_enabled
  end

  test 'mcp widget shows URL when MCP enabled' do
    get settings_path
    assert_response :success
    assert_select '#mcp_url_display'
  end

  test 'mcp widget shows connected clients section' do
    get settings_path
    assert_response :success
    assert_select '#mcp_connected_clients'
  end

  test 'mcp widget shows client when one exists' do
    Doorkeeper::Application.create!(name: 'Test Client', redirect_uri: 'http://localhost/callback', confidential: false)

    get settings_path
    assert_response :success
    assert_select '#mcp_connected_clients', /Test Client/
  end

  test 'revoke client removes application and tokens' do
    app = Doorkeeper::Application.create!(name: 'Test Client', redirect_uri: 'http://localhost/callback', confidential: false)
    Doorkeeper::AccessToken.create!(application: app, resource_owner_id: @admin.id, token: SecureRandom.hex(32), expires_in: 3600)

    assert_difference 'Doorkeeper::Application.count', -1 do
      delete settings_revoke_mcp_client_path(id: app.id)
    end

    assert_response :success
    assert Doorkeeper::AccessToken.where(application_id: app.id).all?(&:revoked?)
  end

  test 'non-admin cannot revoke client' do
    regular_user = create(:user, setup_completed: true)
    sign_in regular_user
    app = Doorkeeper::Application.create!(name: 'Test Client', redirect_uri: 'http://localhost/callback', confidential: false)

    delete settings_revoke_mcp_client_path(id: app.id)
    assert_response :forbidden
  end

  test 'mcp widget is hidden when MCP_ENABLED is not true' do
    ENV['MCP_ENABLED'] = nil

    get settings_path
    assert_response :success
    assert_select 'turbo-frame#mcp_settings', count: 0
  end

  test 'mcp widget is not shown to non-admin users' do
    regular_user = create(:user, setup_completed: true)
    sign_in regular_user

    get settings_path
    assert_response :success
    assert_select 'turbo-frame#mcp_settings', count: 0
  end
end
