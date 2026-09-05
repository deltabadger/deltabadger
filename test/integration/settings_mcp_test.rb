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
    get settings_api_path
    assert_response :success
    assert_select '#mcp_url_display'
  end

  # The page is four frames: the two surfaces on top, then the shared tool matrix, then the
  # clients — each re-rendered on its own, so a switch in one never redraws the others.
  test 'the api page renders the surfaces, the permissions and the clients as separate frames' do
    get settings_api_path
    assert_response :success
    assert_select 'turbo-frame#mcp_settings'
    assert_select 'turbo-frame#rest_settings'
    assert_select 'turbo-frame#tool_permissions'
    assert_select 'turbo-frame#connected_clients'
  end

  test 'connected clients have their own section' do
    get settings_api_path
    assert_response :success
    assert_select 'turbo-frame#connected_clients', /#{I18n.t('settings.mcp.no_clients')}/
  end

  test 'connected clients section shows a client when one exists' do
    app = Doorkeeper::Application.create!(name: 'Test Client', redirect_uri: 'http://localhost/callback', confidential: false)
    Doorkeeper::AccessToken.create!(application: app, resource_owner_id: @admin.id, token: SecureRandom.hex(32), expires_in: 3600)
    ConnectedClient.create!(user: @admin, oauth_application: app, mcp_tools: AppConfig::MCP_TOOL_GROUPS['read'])

    get settings_api_path
    assert_response :success
    assert_select 'turbo-frame#connected_clients .connected-client', /Test Client/
  end

  test 'the paper trading switch posts within the mcp frame' do
    get settings_api_path
    assert_response :success
    assert_select 'turbo-frame#mcp_settings form[action=?][data-turbo-frame=mcp_settings]',
                  settings_update_mcp_dry_run_path do
      assert_select 'input[name=enabled][value=1]', 1, 'off by default, so the switch turns it on'
      assert_select '.toggle input[type=checkbox]:not([checked])', 1
    end
  end

  test 'the mcp widget no longer carries the tool toggles' do
    get settings_api_path
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

  test 'the revoke confirmation posts back into the clients frame' do
    app = Doorkeeper::Application.create!(name: 'Test Client', redirect_uri: 'http://localhost/callback', confidential: false)
    Doorkeeper::AccessToken.create!(application: app, resource_owner_id: @admin.id, token: SecureRandom.hex(32), expires_in: 3600)

    get settings_confirm_revoke_mcp_client_path(id: app.id)

    assert_response :success
    # button_to puts `data:` on the submitter, and Turbo reads the frame from there.
    assert_select 'form[action=?] button[data-turbo-frame=connected_clients]', settings_revoke_mcp_client_path(app)
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

    get settings_api_path
    assert_response :success
    assert_select 'turbo-frame#mcp_settings'
  end

  test 'the MCP url copies through a Stimulus action, not an inline handler' do
    get settings_api_path

    assert_response :success
    assert_select '#mcp_url_display[data-controller=?][data-action=?]',
                  'clipboard', 'click->clipboard#copy'
    assert_select '#mcp_url_display[data-clipboard-text-value=?]', AppConfig.mcp_url
    assert_select '#mcp_url_display[onclick]', false, 'an inline handler cannot run under the policy'
  end
end
