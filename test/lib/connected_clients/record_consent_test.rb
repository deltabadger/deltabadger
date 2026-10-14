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

    test 'a new connection gets a row with no tool lists' do
      client = RecordConsent.call(grant: grant_for('mcp'))

      assert client.persisted?
      assert_nil client.reload.mcp_tools
      assert_nil client.rest_tools
    end

    test 'a following client picks up a global switch at once, both ways' do
      client = RecordConsent.call(grant: grant_for('mcp api'))

      @user.set_mcp_tool_enabled('market_buy', true)
      @user.set_rest_tool_enabled('list_bots', true)
      assert_includes client.reload.granted_mcp_tools, 'market_buy'
      assert_includes client.granted_rest_tools, 'list_bots'

      @user.set_mcp_tool_enabled('market_buy', false)
      assert_not_includes client.reload.granted_mcp_tools, 'market_buy'
    end

    # create_or_find_by! inserts first, so this is also the lost-race path: the unique
    # violation reads the row back instead of raising.
    test 'an existing row is returned untouched' do
      existing = ConnectedClient.create!(user: @user, oauth_application: @application,
                                         mcp_tools: %w[list_bots], rest_tools: [])
      existing.update_columns(updated_at: 1.day.ago)
      before = existing.reload.attributes

      client = RecordConsent.call(grant: grant_for('mcp api'))

      assert_equal existing.id, client.id
      assert_equal before, existing.reload.attributes
    end

    test 'does nothing for something that is not an authorization grant' do
      assert_nil RecordConsent.call(grant: nil)
      assert_no_difference('ConnectedClient.count') { RecordConsent.call(grant: Object.new) }
    end

    test 'does nothing when the grant owner is gone' do
      grant = grant_for('mcp')
      grant.update_column(:resource_owner_id, 0)

      assert_nil RecordConsent.call(grant: grant)
    end
  end
end
