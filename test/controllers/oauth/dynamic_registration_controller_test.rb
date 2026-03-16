require 'test_helper'

class Oauth::DynamicRegistrationControllerTest < ActionDispatch::IntegrationTest
  teardown do
    Doorkeeper::Application.destroy_all
  end

  test 'creates application and returns client_id' do
    assert_difference 'Doorkeeper::Application.count', 1 do
      post '/oauth/register', params: { client_name: 'Claude', redirect_uris: ['http://localhost/callback'] }, as: :json
    end

    assert_response :created

    json = JSON.parse(response.body)
    assert json['client_id'].present?
    assert_equal 'Claude', json['client_name']
    assert_includes json['redirect_uris'], 'http://localhost/callback'
    assert json['registration_access_token'].present?
    assert_equal 'none', json['token_endpoint_auth_method']
  end

  test 'fails without redirect_uris' do
    post '/oauth/register', params: { client_name: 'Claude' }, as: :json
    assert_response :bad_request

    json = JSON.parse(response.body)
    assert_equal 'invalid_client_metadata', json['error']
  end

  test 'uses default client name when not provided' do
    post '/oauth/register', params: { redirect_uris: ['http://localhost/callback'] }, as: :json
    assert_response :created

    json = JSON.parse(response.body)
    assert_equal 'MCP Client', json['client_name']
  end
end
