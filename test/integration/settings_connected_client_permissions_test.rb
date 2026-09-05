require 'test_helper'

class SettingsConnectedClientPermissionsTest < ActionDispatch::IntegrationTest
  include Devise::Test::IntegrationHelpers

  setup do
    @user = create(:user, admin: true, setup_completed: true)
    @application = Doorkeeper::Application.create!(
      name: 'Acme Connector', redirect_uri: 'http://localhost/callback',
      confidential: false, scopes: 'mcp api'
    )
    @token = Doorkeeper::AccessToken.create!(
      application: @application, resource_owner_id: @user.id,
      token: SecureRandom.hex(32), scopes: 'mcp api', expires_in: 3600
    )
    @client = ConnectedClient.create!(
      user: @user, oauth_application: @application,
      mcp_tools: AppConfig::MCP_TOOL_GROUPS['read']
    )
    sign_in @user
  end

  def update_path
    settings_update_client_tool_permissions_path(id: @application.id)
  end

  # ---- the list -----------------------------------------------------------

  test 'lists the connected client' do
    get settings_api_path

    assert_response :success
    assert_select 'turbo-frame#connected_clients .connected-client', /Acme Connector/
  end

  # ---- the card -----------------------------------------------------------
  #
  # A client card is the tool matrix at group level: a row per group, a column per surface.
  # A switch appears only where the client holds the scope AND the surface has the group.

  test 'a client with both scopes gets a switch per surface, except where REST has no group' do
    get settings_api_path

    assert_select '.connected-client' do
      assert_select 'tr[data-group]', AppConfig::MCP_TOOL_GROUPS.size
      %w[read control trade].each do |group|
        assert_select "tr[data-group=#{group}] td[data-surface=mcp] form[action=?]", update_path, 1
        assert_select "tr[data-group=#{group}] td[data-surface=rest] form[action=?]", update_path, 1 do
          assert_select 'input[name=surface][value=rest]', 1
          assert_select 'input[name=group][value=?]', group, 1
        end
      end
      assert_select 'tr[data-group=tax] td[data-surface=mcp] form', 1
      assert_select 'tr[data-group=tax] td[data-surface=rest]', 1 do
        assert_select 'form', 0
      end
    end
  end

  test 'a client with only the mcp scope keeps the REST column, empty' do
    @application.update!(scopes: 'mcp')
    @token.update!(scopes: 'mcp')
    get settings_api_path

    assert_select '.connected-client' do
      assert_select 'td[data-surface=rest]', AppConfig::MCP_TOOL_GROUPS.size
      assert_select 'td[data-surface=rest] form', 0
      assert_select 'td[data-surface=mcp] form', AppConfig::MCP_TOOL_GROUPS.size
    end
  end

  test 'a fully granted group is a checked switch that revokes' do
    get settings_api_path

    assert_select '.connected-client tr[data-group=read] td[data-surface=mcp]' do
      assert_select '.toggle:not(.toggle--partial) input[type=checkbox][checked]', 1
      assert_select 'input[name=enabled][value=0]', 1
    end
  end

  test 'a group the client holds only part of renders as partial' do
    @client.update!(mcp_tools: %w[list_bots])
    get settings_api_path

    assert_select '.connected-client tr[data-group=read] td[data-surface=mcp]' do
      assert_select '.toggle.toggle--partial input[type=checkbox][checked]', 1
      assert_select 'input[name=enabled][value=0]', 1, 'partial reads as on, so the click revokes'
    end
  end

  test 'a group the client has none of is an unchecked switch that grants' do
    get settings_api_path

    assert_select '.connected-client tr[data-group=trade] td[data-surface=mcp]' do
      assert_select '.toggle:not(.toggle--partial) input[type=checkbox]:not([checked])', 1
      assert_select 'input[name=enabled][value=1]', 1
    end
  end

  # The user's own switch moved the group's ceiling; the card that read ON a moment ago must now
  # read PARTIAL without a reload, or it claims a grant the client never got.
  test 'enabling a tool the client lacks turns its card from on to partial in the same response' do
    AppConfig::MCP_TOOL_GROUPS['read'].each { |t| @user.set_mcp_tool_enabled(t, false) }
    @user.set_mcp_tool_enabled('list_bots', true)
    @client.update!(mcp_tools: %w[list_bots])

    get settings_api_path
    assert_select '.connected-client tr[data-group=read] td[data-surface=mcp] .toggle:not(.toggle--partial) input[checked]', 1

    patch settings_update_mcp_tool_permissions_path, params: { tool_name: 'list_exchanges', enabled: '1' }

    assert_response :success
    assert_select 'turbo-stream[target=connected_clients]' do |streams|
      card = Nokogiri::HTML5.fragment(streams.first.at('template').inner_html)
      assert_equal 1, card.css('.connected-client tr[data-group=read] td[data-surface=mcp] .toggle--partial').size,
                   'the card did not learn that the group grew past the grant'
    end
  end

  test 'editing a grant re-renders the clients frame only' do
    patch update_path, params: { surface: 'mcp', group: 'tax', enabled: '1' }

    assert_response :success
    assert_select 'turbo-stream[action=replace][target=connected_clients]', 1
    assert_select 'turbo-stream[target=mcp_settings]', 0
  end

  test 'a client whose tokens are all revoked drops off the list' do
    @token.update!(revoked_at: Time.current)

    get settings_api_path

    assert_response :success
    assert_no_match(/Acme Connector/, response.body)
  end

  test 'a client with a live token but no grant record is still listed and revokable' do
    @client.destroy!

    get settings_api_path

    assert_response :success
    assert_match 'Acme Connector', response.body

    assert_difference 'Doorkeeper::Application.count', -1 do
      delete settings_revoke_mcp_client_path(id: @application.id)
    end
  end

  test 'a client holding only an unredeemed authorization code is listed and revokable' do
    @token.destroy!
    @client.destroy!
    grant = Doorkeeper::AccessGrant.create!(
      application: @application, resource_owner_id: @user.id,
      token: SecureRandom.hex(32), expires_in: 600,
      redirect_uri: 'http://localhost/callback', scopes: 'mcp'
    )

    get settings_api_path

    assert_match 'Acme Connector', response.body

    delete settings_revoke_mcp_client_path(id: @application.id)

    assert_not Doorkeeper::AccessGrant.exists?(grant.id), 'the application and its grant are gone'
  end

  test 'a client whose only authorization code has expired is not listed' do
    @token.destroy!
    Doorkeeper::AccessGrant.create!(
      application: @application, resource_owner_id: @user.id,
      token: SecureRandom.hex(32), expires_in: 600, created_at: 2.hours.ago,
      redirect_uri: 'http://localhost/callback', scopes: 'mcp'
    )

    get settings_api_path

    assert_no_match(/Acme Connector/, response.body)
  end

  test 'the revoke confirmation modal renders' do
    get settings_confirm_revoke_mcp_client_path(id: @application.id)

    assert_response :success
    assert_match 'Acme Connector', response.body
  end

  # ---- editing a grant ----------------------------------------------------

  test 'granting a group adds only tools the user has on' do
    patch update_path, params: { surface: 'mcp', group: 'tax', enabled: '1' }

    granted = @client.reload.granted_mcp_tools
    assert_includes granted, 'download_tax_report'
    assert_equal @user.enabled_mcp_tool_names & AppConfig::MCP_TOOL_GROUPS['tax'],
                 granted & AppConfig::MCP_TOOL_GROUPS['tax']
  end

  test 'revoking a group removes it' do
    patch update_path, params: { surface: 'mcp', group: 'read', enabled: '0' }

    assert_empty @client.reload.granted_mcp_tools
  end

  test 'a residual grant stays revokable after the user turns the tool off globally' do
    # The group toggle must not go dead just because the user's own set moved,
    # or the grant sits there dormant and re-arms when the tool comes back.
    AppConfig::MCP_TOOL_GROUPS['read'].each { |t| @user.set_mcp_tool_enabled(t, false) }

    patch update_path, params: { surface: 'mcp', group: 'read', enabled: '0' }

    assert_empty @client.reload.granted_mcp_tools
  end

  test 'rest grants have their own toggles' do
    @user.set_rest_tool_enabled('list_bots', true)
    before = @client.granted_mcp_tools

    patch update_path, params: { surface: 'rest', group: 'read', enabled: '1' }

    assert_includes @client.reload.granted_rest_tools, 'list_bots'
    assert_equal before, @client.granted_mcp_tools, 'the mcp grant must not be touched'
  end

  test 'a group grant can never exceed the user own set' do
    @user.set_mcp_tool_enabled('download_tax_report', false)

    patch update_path, params: { surface: 'mcp', group: 'tax', enabled: '1' }

    assert_not_includes @client.reload.granted_mcp_tools, 'download_tax_report'
  end

  test 'rejects a group that is not a real group' do
    patch update_path, params: { surface: 'mcp', group: 'everything', enabled: '1' }

    assert_response :unprocessable_entity
  end

  test 'rejects a surface that is not a real surface' do
    patch update_path, params: { surface: 'telepathy', group: 'read', enabled: '1' }

    assert_response :unprocessable_entity
  end

  test 'cannot touch another user client' do
    other_app = Doorkeeper::Application.create!(
      name: 'Theirs', redirect_uri: 'http://localhost/x', confidential: false, scopes: 'mcp'
    )
    other = create(:user, setup_completed: true)
    theirs = ConnectedClient.create!(user: other, oauth_application: other_app)

    patch settings_update_client_tool_permissions_path(id: other_app.id),
          params: { surface: 'mcp', group: 'read', enabled: '1' }

    assert_response :not_found
    assert_empty theirs.reload.granted_mcp_tools
  end

  test 'toggling two groups in sequence accumulates rather than clobbering' do
    # The action rewrites the whole JSON column, so it re-reads under a row lock.
    # If it ever went back to writing a snapshot taken before the lock, the second
    # request would drop the first group.
    patch update_path, params: { surface: 'mcp', group: 'tax', enabled: '1' }
    patch update_path, params: { surface: 'mcp', group: 'control', enabled: '1' }

    granted = @client.reload.granted_mcp_tools
    assert_includes granted, 'download_tax_report'
    assert_equal AppConfig::MCP_TOOL_GROUPS['read'], granted & AppConfig::MCP_TOOL_GROUPS['read']
  end

  # ---- revoke -------------------------------------------------------------

  test 'revoking disconnects this user only and leaves other users connected' do
    other = create(:user, setup_completed: true)
    ConnectedClient.create!(user: other, oauth_application: @application, mcp_tools: %w[list_bots])
    theirs = Doorkeeper::AccessToken.create!(
      application: @application, resource_owner_id: other.id,
      token: SecureRandom.hex(32), scopes: 'mcp', expires_in: 3600
    )

    assert_difference 'ConnectedClient.count', -1 do
      delete settings_revoke_mcp_client_path(id: @application.id)
    end

    assert @token.reload.revoked?
    assert_not theirs.reload.revoked?
    assert Doorkeeper::Application.exists?(@application.id), 'the other user is still connected'
  end

  test 'revoking keeps the application when another user has a live token but no grant record' do
    other = create(:user, setup_completed: true)
    theirs = Doorkeeper::AccessToken.create!(
      application: @application, resource_owner_id: other.id,
      token: SecureRandom.hex(32), scopes: 'mcp', expires_in: 3600
    )

    delete settings_revoke_mcp_client_path(id: @application.id)

    assert_not theirs.reload.revoked?
    assert Doorkeeper::Application.exists?(@application.id)
  end

  test 'revoking the last connection removes the application row' do
    assert_difference 'Doorkeeper::Application.count', -1 do
      delete settings_revoke_mcp_client_path(id: @application.id)
    end
  end

  test 'another user expired authorization code does not keep the application alive' do
    # An expired code is dead: it cannot be redeemed and its owner is not listed as
    # connected. Counting it as a live credential would strand the application row
    # forever on any instance where someone once abandoned a consent flow.
    other = create(:user, setup_completed: true)
    Doorkeeper::AccessGrant.create!(
      application: @application, resource_owner_id: other.id,
      token: SecureRandom.hex(32), expires_in: 600, created_at: 2.hours.ago,
      redirect_uri: 'http://localhost/callback', scopes: 'mcp'
    )

    assert_difference 'Doorkeeper::Application.count', -1 do
      delete settings_revoke_mcp_client_path(id: @application.id)
    end
  end

  test 'revoking also kills unredeemed authorization codes' do
    other = create(:user, setup_completed: true)
    Doorkeeper::AccessToken.create!(
      application: @application, resource_owner_id: other.id,
      token: SecureRandom.hex(32), scopes: 'mcp', expires_in: 3600
    )
    grant = Doorkeeper::AccessGrant.create!(
      application: @application, resource_owner_id: @user.id,
      token: SecureRandom.hex(32), expires_in: 600,
      redirect_uri: 'http://localhost/callback', scopes: 'mcp'
    )

    delete settings_revoke_mcp_client_path(id: @application.id)

    assert grant.reload.revoked?
  end
end
