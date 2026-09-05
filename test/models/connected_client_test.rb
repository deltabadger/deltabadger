require 'test_helper'

class ConnectedClientTest < ActiveSupport::TestCase
  setup do
    @user = create(:user)
    @application = Doorkeeper::Application.create!(
      name: 'Test client', redirect_uri: 'http://localhost/callback',
      confidential: false, scopes: 'mcp'
    )
  end

  test 'stores granted tool names per surface' do
    client = ConnectedClient.create!(
      user: @user, oauth_application: @application,
      mcp_tools: %w[list_bots get_portfolio_summary], rest_tools: %w[list_bots]
    )

    assert_equal %w[list_bots get_portfolio_summary], client.granted_mcp_tools
    assert_equal %w[list_bots], client.granted_rest_tools
  end

  test 'defaults both granted sets to empty' do
    client = ConnectedClient.create!(user: @user, oauth_application: @application)

    assert_empty client.granted_mcp_tools
    assert_empty client.granted_rest_tools
  end

  test 'filters tool names that are not real tools' do
    client = ConnectedClient.create!(
      user: @user, oauth_application: @application,
      mcp_tools: %w[list_bots renamed_away], rest_tools: %w[list_bots renamed_away]
    )

    assert_equal %w[list_bots], client.granted_mcp_tools
    assert_equal %w[list_bots], client.granted_rest_tools
  end

  test 'one record per user and application' do
    ConnectedClient.create!(user: @user, oauth_application: @application)

    assert_raises(ActiveRecord::RecordNotUnique) do
      ConnectedClient.new(user: @user, oauth_application: @application).save!(validate: false)
    end
  end

  test 'two users may connect the same application independently' do
    other = create(:user)
    ConnectedClient.create!(user: @user, oauth_application: @application, mcp_tools: %w[list_bots])
    ConnectedClient.create!(user: other, oauth_application: @application, mcp_tools: %w[market_buy])

    assert_equal %w[list_bots], ConnectedClient.for(user: @user, application: @application).granted_mcp_tools
    assert_equal %w[market_buy], ConnectedClient.for(user: other, application: @application).granted_mcp_tools
  end

  test 'for returns nil when the pair has no record' do
    assert_nil ConnectedClient.for(user: @user, application: @application)
  end

  test 'destroying the user destroys the record' do
    ConnectedClient.create!(user: @user, oauth_application: @application)

    assert_difference 'ConnectedClient.count', -1 do
      @user.destroy!
    end
  end

  test 'destroying the application destroys the record' do
    ConnectedClient.create!(user: @user, oauth_application: @application)

    assert_difference 'ConnectedClient.count', -1 do
      @application.destroy!
    end
  end
end
