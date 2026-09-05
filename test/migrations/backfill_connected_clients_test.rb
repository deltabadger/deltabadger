require 'test_helper'
require Dir[Rails.root.join('db/migrate/*_backfill_connected_clients.rb')].first

class BackfillConnectedClientsTest < ActiveSupport::TestCase
  setup do
    ActiveRecord::Migration.verbose = false
    @user = create(:user)
    @application = Doorkeeper::Application.create!(
      name: 'Existing client', redirect_uri: 'http://localhost/callback',
      confidential: false, scopes: 'mcp'
    )
  end

  def backfill = BackfillConnectedClients.new.up

  def token_for(application, user, scopes: 'mcp', **attrs)
    Doorkeeper::AccessToken.create!(
      { application: application, resource_owner_id: user.id,
        token: SecureRandom.hex(32), scopes: scopes, expires_in: 3600 }.merge(attrs)
    )
  end

  def grant_for(application, user, scopes: 'mcp', **attrs)
    Doorkeeper::AccessGrant.create!(
      { application: application, resource_owner_id: user.id,
        token: SecureRandom.hex(32), expires_in: 600,
        redirect_uri: 'http://localhost/callback', scopes: scopes }.merge(attrs)
    )
  end

  test 'snapshots the user current set for each existing connection' do
    token_for(@application, @user)

    assert_difference 'ConnectedClient.count', 1 do
      assert_equal 1, backfill
    end

    client = ConnectedClient.for(user: @user, application: @application)
    # Against the migration's own frozen list, not the live catalogue: the snapshot records what
    # the connection could do when this ran, so a tool added later must not appear in it.
    expected = BackfillConnectedClients::MCP_DEFAULTS.select { |_, on| on }.keys
    assert_equal expected.sort, client.granted_mcp_tools.sort
  end

  test 'an expired access token is still a live connection' do
    # Tokens expire hourly and refresh forever, so "expired" is the resting state
    # of every idle connector. Skipping these would disconnect them all.
    token_for(@application, @user, expires_in: 0, created_at: 2.hours.ago)

    assert_difference 'ConnectedClient.count', 1 do
      backfill
    end
  end

  test 'an unredeemed authorization code is a live connection' do
    grant_for(@application, @user)

    assert_difference 'ConnectedClient.count', 1 do
      backfill
    end
  end

  test 'an expired authorization code is not' do
    grant_for(@application, @user, expires_in: 600, created_at: 2.hours.ago)

    assert_no_difference 'ConnectedClient.count' do
      backfill
    end
  end

  test 'aggregates scopes across every credential for the pair' do
    # One client, two separate authorizations. Recording only the first would
    # silently drop the other surface.
    @user.set_rest_tool_enabled('list_bots', true)
    token_for(@application, @user, scopes: 'mcp')
    token_for(@application, @user, scopes: 'api')

    assert_difference 'ConnectedClient.count', 1 do
      backfill
    end

    client = ConnectedClient.for(user: @user, application: @application)
    assert_not_empty client.granted_mcp_tools
    assert_includes client.granted_rest_tools, 'list_bots'
  end

  test 'only grants the surfaces the credentials actually hold' do
    @user.set_rest_tool_enabled('list_bots', true)
    token_for(@application, @user, scopes: 'mcp')

    backfill

    client = ConnectedClient.for(user: @user, application: @application)
    assert_not_empty client.granted_mcp_tools
    assert_empty client.granted_rest_tools
  end

  test 'honours a per-user override rather than the bare defaults' do
    @user.set_mcp_tool_enabled('list_bots', false)
    @user.set_mcp_tool_enabled('market_buy', true)
    token_for(@application, @user)

    backfill

    granted = ConnectedClient.for(user: @user, application: @application).granted_mcp_tools
    assert_not_includes granted, 'list_bots'
    assert_includes granted, 'market_buy'
  end

  test 'skips the personal api token application' do
    @user.personal_api_token

    assert_no_difference 'ConnectedClient.count' do
      backfill
    end
  end

  test 'leaves the real personal api token alone' do
    token = @user.personal_api_token

    # These two nils are the discriminator the migration keys on: mint_personal_token!
    # builds the token with a plain create!, so it gets neither the configured 1h
    # expiry nor a refresh token, and every OAuth-issued credential gets both.
    assert_nil token.expires_in
    assert_nil token.refresh_token

    backfill

    assert_not token.reload.revoked?
  end

  test 'revokes a token minted against the personal application through the oauth flow' do
    # A token on a personal application takes ToolAccess's "acting as self" branch and
    # appears in no client list. Keeping these applications out of the authorization
    # flow governs new credentials only, so anything already issued is retired here.
    personal = @user.personal_api_token.application
    smuggled = Doorkeeper::AccessToken.create!(
      application: personal, resource_owner_id: @user.id,
      token: SecureRandom.hex(32), refresh_token: SecureRandom.hex(32),
      scopes: 'api', expires_in: 3600
    )

    backfill

    assert smuggled.reload.revoked?
    assert_not @user.personal_api_token.revoked?, 'the legitimate token survives'
  end

  test 'revokes any authorization code on the personal application' do
    personal = @user.personal_api_token.application
    grant = Doorkeeper::AccessGrant.create!(
      application: personal, resource_owner_id: @user.id,
      token: SecureRandom.hex(32), expires_in: 600,
      redirect_uri: 'https://localhost/personal-access-token', scopes: 'api'
    )

    backfill

    assert grant.reload.revoked?
  end

  test 'skips revoked credentials' do
    token_for(@application, @user, revoked_at: Time.current)
    grant_for(@application, @user, revoked_at: Time.current)

    assert_no_difference 'ConnectedClient.count' do
      backfill
    end
  end

  test 'one record per user and application even with several tokens' do
    3.times { token_for(@application, @user) }

    assert_difference 'ConnectedClient.count', 1 do
      backfill
    end
  end

  test 'gives each user of a shared client their own snapshot' do
    other = create(:user)
    other.set_mcp_tool_enabled('list_bots', false)
    token_for(@application, @user)
    token_for(@application, other)

    assert_difference 'ConnectedClient.count', 2 do
      backfill
    end

    assert_includes ConnectedClient.for(user: @user, application: @application).granted_mcp_tools, 'list_bots'
    assert_not_includes ConnectedClient.for(user: other, application: @application).granted_mcp_tools, 'list_bots'
  end

  test 'is safe to run twice and never overwrites an existing grant' do
    token_for(@application, @user)
    backfill
    ConnectedClient.for(user: @user, application: @application).update!(mcp_tools: %w[list_bots])

    assert_no_difference 'ConnectedClient.count' do
      assert_equal 0, backfill
    end

    assert_equal %w[list_bots], ConnectedClient.for(user: @user, application: @application).granted_mcp_tools
  end

  test 'skips a credential whose user is gone' do
    token_for(@application, @user).update_column(:resource_owner_id, 0)

    assert_no_difference 'ConnectedClient.count' do
      backfill
    end
  end

  test 'skips a credential whose application is gone' do
    # No database FK exists on oauth_access_tokens, so a dangling application_id is
    # possible — and inserting against it would violate the new FK on
    # connected_clients and take the whole migration, and the container boot, down.
    token_for(@application, @user).update_column(:application_id, 999_999)

    assert_nothing_raised do
      assert_no_difference 'ConnectedClient.count' do
        backfill
      end
    end
  end
end
