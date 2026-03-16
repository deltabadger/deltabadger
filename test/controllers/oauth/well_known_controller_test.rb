require 'test_helper'

class Oauth::WellKnownControllerTest < ActionDispatch::IntegrationTest
  test 'protected resource returns valid RFC 9728 JSON' do
    get '/.well-known/oauth-protected-resource'
    assert_response :success

    json = JSON.parse(response.body)
    assert_equal 'http://www.example.com', json['resource']
    assert_includes json['authorization_servers'], 'http://www.example.com'
    assert_includes json['bearer_methods_supported'], 'header'
  end

  test 'authorization server returns valid RFC 8414 JSON' do
    get '/.well-known/oauth-authorization-server'
    assert_response :success

    json = JSON.parse(response.body)
    assert_equal 'http://www.example.com', json['issuer']
    assert_equal 'http://www.example.com/oauth/authorize', json['authorization_endpoint']
    assert_equal 'http://www.example.com/oauth/token', json['token_endpoint']
    assert_equal 'http://www.example.com/oauth/register', json['registration_endpoint']
    assert_equal 'http://www.example.com/oauth/revoke', json['revocation_endpoint']
    assert_includes json['scopes_supported'], 'mcp'
    assert_includes json['response_types_supported'], 'code'
    assert_includes json['grant_types_supported'], 'authorization_code'
    assert_includes json['token_endpoint_auth_methods_supported'], 'none'
    assert_includes json['code_challenge_methods_supported'], 'S256'
  end
end
