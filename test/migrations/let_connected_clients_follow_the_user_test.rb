require 'test_helper'
require Dir[Rails.root.join('db/migrate/*_let_connected_clients_follow_the_user.rb')].first

class LetConnectedClientsFollowTheUserTest < ActiveSupport::TestCase
  setup do
    ActiveRecord::Migration.verbose = false
    @user = create(:user)
    @application = Doorkeeper::Application.create!(
      name: 'Existing client', redirect_uri: 'http://localhost/callback',
      confidential: false, scopes: 'mcp'
    )
  end

  def up = LetConnectedClientsFollowTheUser.new.up
  def down = LetConnectedClientsFollowTheUser.new.down

  def column_nullable?
    ConnectedClient.connection.columns(:connected_clients).find { |c| c.name == 'mcp_tools' }.null
  end

  def token_for(**attrs)
    Doorkeeper::AccessToken.create!({ application: @application, resource_owner_id: @user.id,
                                      token: SecureRandom.hex(32), scopes: 'mcp', expires_in: 3600 }.merge(attrs))
  end

  def grant_for(**attrs)
    Doorkeeper::AccessGrant.create!({ application: @application, resource_owner_id: @user.id,
                                      token: SecureRandom.hex(32), expires_in: 600,
                                      redirect_uri: 'http://localhost/callback', scopes: 'mcp' }.merge(attrs))
  end

  def client = ConnectedClient.for(user: @user, application: @application)

  test 'a row-less client with a live token gets an explicit empty list' do
    token_for(expires_in: 0, created_at: 2.hours.ago)

    up

    assert_equal [[], []], [client.mcp_tools, client.rest_tools]
    assert_empty client.granted_mcp_tools
  end

  test 'a row-less client holding a live authorization code gets an explicit empty list' do
    grant_for

    up

    assert_equal [], client.mcp_tools
  end

  test 'revoked or expired credentials alone get no row' do
    token_for(revoked_at: Time.current)
    grant_for(revoked_at: Time.current)
    grant_for(created_at: 2.hours.ago)

    assert_no_difference('ConnectedClient.count') { up }
  end

  test 'an existing row and the personal application are left alone' do
    token_for
    ConnectedClient.create!(user: @user, oauth_application: @application, mcp_tools: %w[list_bots])
    @user.personal_api_token

    assert_no_difference('ConnectedClient.count') { up }
    assert_equal %w[list_bots], client.mcp_tools
  end

  test 'after the backfill, consent gives a row-less client NULL only when it is new' do
    token_for
    other = Doorkeeper::Application.create!(name: 'New client', redirect_uri: 'http://localhost/callback',
                                            confidential: false, scopes: 'mcp')
    up

    [@application, other].each do |application|
      grant = Doorkeeper::AccessGrant.create!(application: application, resource_owner_id: @user.id,
                                              token: SecureRandom.hex(32), expires_in: 600,
                                              redirect_uri: 'http://localhost/callback', scopes: 'mcp')
      ConnectedClients::RecordConsent.call(grant: grant)
    end

    assert_equal [], client.mcp_tools, 'the client that already held a token stays as it was'
    assert_nil ConnectedClient.for(user: @user, application: other).mcp_tools
  end

  test 'rolling back turns NULL into an empty list and keeps explicit lists and the default' do
    following = ConnectedClient.create!(user: @user, oauth_application: @application, mcp_tools: nil, rest_tools: nil)
    other = Doorkeeper::Application.create!(name: 'Other', redirect_uri: 'http://localhost/callback',
                                            confidential: false, scopes: 'mcp')
    pinned = ConnectedClient.create!(user: @user, oauth_application: other, mcp_tools: %w[list_bots], rest_tools: nil)

    down
    ConnectedClient.reset_column_information

    assert_not column_nullable?
    following.reload
    assert_equal [[], []], [following.mcp_tools, following.rest_tools]
    pinned.reload
    assert_equal [%w[list_bots], []], [pinned.mcp_tools, pinned.rest_tools]
    assert_equal [], ConnectedClient.new.mcp_tools
  ensure
    ConnectedClient.reset_column_information
  end
end
