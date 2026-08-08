require 'test_helper'

module ConnectedClients
  class RecordConsentTest < ActiveSupport::TestCase
    setup do
      @user = create(:user)
      @application = Doorkeeper::Application.create!(
        name: 'Test client', redirect_uri: 'http://localhost/callback',
        confidential: false, scopes: 'mcp api'
      )
    end

    def grant_for(scopes)
      Doorkeeper::AccessGrant.create!(
        application: @application, resource_owner_id: @user.id,
        token: SecureRandom.hex(32), expires_in: 600,
        redirect_uri: 'http://localhost/callback', scopes: scopes
      )
    end

    test 'grants the checked groups intersected with what the user has on' do
      client = RecordConsent.call(grant: grant_for('mcp'), mcp_groups: %w[read], rest_groups: [])

      assert_equal @user.enabled_mcp_tool_names & AppConfig::MCP_TOOL_GROUPS['read'],
                   client.granted_mcp_tools
      assert_empty client.granted_rest_tools
    end

    test 'an unchecked group grants nothing from it' do
      client = RecordConsent.call(grant: grant_for('mcp'), mcp_groups: %w[read], rest_groups: [])

      assert_not_includes client.granted_mcp_tools, 'download_tax_report'
    end

    test 'never grants a tool the user has turned off' do
      @user.set_mcp_tool_enabled('list_bots', false)

      client = RecordConsent.call(grant: grant_for('mcp'), mcp_groups: %w[read], rest_groups: [])

      assert_not_includes client.granted_mcp_tools, 'list_bots'
    end

    test 'ignores a group that is not a real group' do
      client = RecordConsent.call(grant: grant_for('mcp'), mcp_groups: %w[read everything], rest_groups: [])

      assert_equal @user.enabled_mcp_tool_names & AppConfig::MCP_TOOL_GROUPS['read'],
                   client.granted_mcp_tools
    end

    test 'records both surfaces for a dual-scope grant' do
      @user.set_rest_tool_enabled('list_bots', true)

      client = RecordConsent.call(grant: grant_for('mcp api'), mcp_groups: %w[read], rest_groups: %w[read])

      assert_includes client.granted_mcp_tools, 'list_bots'
      assert_includes client.granted_rest_tools, 'list_bots'
    end

    test 'a scope the grant does not carry leaves that surface untouched' do
      @user.set_rest_tool_enabled('list_bots', true)
      RecordConsent.call(grant: grant_for('mcp api'), mcp_groups: %w[read], rest_groups: %w[read])

      # Re-authorizing for mcp only must not silently wipe a REST grant the user
      # cannot re-create: nothing in the UI can start an api-scoped authorization.
      client = RecordConsent.call(grant: grant_for('mcp'), mcp_groups: %w[read], rest_groups: [])

      assert_includes client.granted_rest_tools, 'list_bots'
    end

    test 're-consenting replaces the grant for the surfaces it covers' do
      RecordConsent.call(grant: grant_for('mcp'), mcp_groups: %w[read tax], rest_groups: [])
      client = RecordConsent.call(grant: grant_for('mcp'), mcp_groups: %w[read], rest_groups: [])

      assert_equal 1, ConnectedClient.where(user: @user, oauth_application: @application).count
      assert_not_includes client.granted_mcp_tools, 'download_tax_report'
    end

    test 'does nothing when handed something that is not an access grant' do
      assert_no_difference 'ConnectedClient.count' do
        assert_nil RecordConsent.call(grant: nil, mcp_groups: %w[read], rest_groups: [])
        assert_nil RecordConsent.call(grant: 'nonsense', mcp_groups: %w[read], rest_groups: [])
      end
    end

    test 'does nothing when the grant has no resolvable user' do
      grant = grant_for('mcp')
      grant.update_column(:resource_owner_id, 0)

      assert_no_difference 'ConnectedClient.count' do
        assert_nil RecordConsent.call(grant: grant, mcp_groups: %w[read], rest_groups: [])
      end
    end

    test 'survives a concurrent first consent for the same pair' do
      # Two tabs, or a double-submitted Connect button. The loser must not 500
      # after the authorization code has already been committed.
      ConnectedClient.create!(user: @user, oauth_application: @application, mcp_tools: [])
      ConnectedClient.any_instance.stubs(:save!).raises(ActiveRecord::RecordNotUnique).then.returns(true)

      assert_nothing_raised do
        RecordConsent.call(grant: grant_for('mcp'), mcp_groups: %w[read], rest_groups: [])
      end
    end
  end
end
