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
    get settings_mcp_path
    assert_response :success
    assert_select '#mcp_url_display'
  end

  # Each section is three frames: the surface, its tools, its clients — each re-rendered on its
  # own, so a switch in one never redraws the others.
  test 'each surface section renders the surface, the permissions and the clients as separate frames' do
    { settings_mcp_path => 'mcp_settings', settings_api_path => 'rest_settings' }.each do |path, frame|
      get path
      assert_response :success
      assert_select "turbo-frame##{frame}"
      assert_select 'turbo-frame#tool_permissions'
      assert_select 'turbo-frame#connected_clients'
    end
  end

  test 'connected clients have their own section' do
    get settings_mcp_path
    assert_response :success
    assert_select 'turbo-frame#connected_clients', /#{I18n.t('settings.mcp.no_clients')}/
  end

  test 'connected clients section shows a client when one exists' do
    app = Doorkeeper::Application.create!(name: 'Test Client', redirect_uri: 'http://localhost/callback', confidential: false)
    Doorkeeper::AccessToken.create!(application: app, resource_owner_id: @admin.id, token: SecureRandom.hex(32), expires_in: 3600)
    ConnectedClient.create!(user: @admin, oauth_application: app, mcp_tools: AppConfig::MCP_TOOL_GROUPS['read'])

    get settings_mcp_path
    assert_response :success
    assert_select 'turbo-frame#connected_clients .sclient', /Test Client/
  end

  test 'the paper trading switch posts within the mcp frame' do
    get settings_mcp_path
    assert_response :success
    assert_select 'turbo-frame#mcp_settings form[action=?][data-turbo-frame=mcp_settings]',
                  settings_update_mcp_dry_run_path do
      assert_select 'input[name=enabled][value=1]', 1, 'off by default, so the switch turns it on'
      assert_select '.toggle input[type=checkbox]:not([checked])', 1
    end
  end

  test 'the mcp widget no longer carries the tool toggles' do
    get settings_mcp_path
    assert_response :success
    assert_select 'turbo-frame#mcp_settings input[name=tool_name]', 0
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
    assert_select 'turbo-stream[action=replace][target=connected_clients]', 1
  end

  # The dialog is the modal frame, so revoke cannot open a modal of its own: it asks in place.
  test 'revoke asks first, in place' do
    app = Doorkeeper::Application.create!(name: 'Test Client', redirect_uri: 'http://localhost/callback', confidential: false)
    Doorkeeper::AccessToken.create!(application: app, resource_owner_id: @admin.id, token: SecureRandom.hex(32), expires_in: 3600)

    get settings_mcp_path

    assert_response :success
    assert_select 'turbo-frame#connected_clients details.sconfirm' do
      assert_select 'summary', I18n.t('settings.mcp.revoke_client')
      assert_select 'form[action=?]', settings_revoke_mcp_client_path(app)
    end
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

    get settings_mcp_path
    assert_response :success
    assert_select 'turbo-frame#mcp_settings'
  end

  test 'the MCP url copies through a Stimulus action, not an inline handler' do
    get settings_mcp_path

    assert_response :success
    assert_select '#mcp_url_display[data-controller=?][data-action=?]',
                  'clipboard', 'click->clipboard#copy'
    assert_select '#mcp_url_display[data-clipboard-text-value=?]', AppConfig.mcp_url
    assert_select '#mcp_url_display[onclick]', false, 'an inline handler cannot run under the policy'
  end
end
