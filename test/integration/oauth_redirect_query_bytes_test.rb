# frozen_string_literal: true

require 'test_helper'

# A registered redirect URI whose own query holds a byte that is not text (`%FF`) made the approval
# raise (500) while Doorkeeper rebuilt the query. The byte is written back as it stood, as the Rust
# port writes it.
class OauthRedirectQueryBytesTest < ActionDispatch::IntegrationTest
  REDIRECT = 'https://client.example/cb?a+b=c%2Fd&e=%FF'

  setup do
    create(:user, admin: true, setup_completed: true)
    sign_in create(:user, setup_completed: true)
    @application = Doorkeeper::Application.create!(
      name: 'Client', redirect_uri: REDIRECT, confidential: false, scopes: 'mcp',
      token_endpoint_auth_method: 'none', grant_types: 'authorization_code', response_types: 'code'
    )
  end

  test 'the approval redirects with the byte kept' do
    post '/oauth/authorize', params: {
      client_id: @application.uid, redirect_uri: REDIRECT, response_type: 'code', scope: 'mcp',
      code_challenge: 'E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM', code_challenge_method: 'S256'
    }

    assert_response :redirect
    assert_match %r{\Ahttps://client\.example/cb\?a\+b=c%2Fd&e=%FF&code=[^&]+\z}, response.location
  end

  test 'the builder writes the byte back as it stood' do
    assert_equal 'https://client.example/cb?code=c0de&a+b=c%2Fd&e=%FF',
                 Doorkeeper::OAuth::Authorization::URIBuilder.uri_with_query(
                   'https://client.example/cb?code=theirs&a+b=c%2Fd&e=%FF', code: 'c0de'
                 )
  end
end
