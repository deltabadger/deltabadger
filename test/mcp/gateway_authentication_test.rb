require 'test_helper'

class GatewayAuthenticationTest < ActiveSupport::TestCase
  test 'resolves admin user' do
    admin = create(:user, admin: true)

    identifier = MCPTokenIdentifier.new(nil)
    user = identifier.resolve

    assert_equal admin, user
  end

  test 'raises Unauthorized when no admin user exists' do
    identifier = MCPTokenIdentifier.new(nil)

    error = assert_raises(ActionMCP::GatewayIdentifier::Unauthorized) { identifier.resolve }
    assert_match(/No admin user found/, error.message)
  end
end
