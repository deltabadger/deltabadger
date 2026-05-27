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

  # ---- scope handling -----------------------------------------------------

  test 'defaults to mcp scope when scope param is absent (preserves existing behavior)' do
    post '/oauth/register', params: { redirect_uris: ['http://localhost/callback'] }, as: :json
    assert_response :created

    json = JSON.parse(response.body)
    assert_equal 'mcp', json['scope']

    app = Doorkeeper::Application.find_by(uid: json['client_id'])
    assert_equal 'mcp', app.scopes.to_s
  end

  test 'defaults to mcp scope when scope param is blank' do
    post '/oauth/register',
         params: { redirect_uris: ['http://localhost/callback'], scope: '' },
         as: :json
    assert_response :created

    json = JSON.parse(response.body)
    assert_equal 'mcp', json['scope']
  end

  test 'accepts scope=mcp explicitly and reflects it back' do
    post '/oauth/register',
         params: { redirect_uris: ['http://localhost/callback'], scope: 'mcp' },
         as: :json
    assert_response :created

    json = JSON.parse(response.body)
    assert_equal 'mcp', json['scope']
  end

  test 'accepts scope=api and reflects it back' do
    post '/oauth/register',
         params: { redirect_uris: ['http://localhost/callback'], scope: 'api' },
         as: :json
    assert_response :created

    json = JSON.parse(response.body)
    assert_equal 'api', json['scope']

    app = Doorkeeper::Application.find_by(uid: json['client_id'])
    assert_equal 'api', app.scopes.to_s
  end

  test 'accepts combined scope=api mcp' do
    post '/oauth/register',
         params: { redirect_uris: ['http://localhost/callback'], scope: 'api mcp' },
         as: :json
    assert_response :created

    json = JSON.parse(response.body)
    granted = json['scope'].split.sort
    assert_equal %w[api mcp], granted

    app = Doorkeeper::Application.find_by(uid: json['client_id'])
    assert_equal %w[api mcp], app.scopes.to_a.sort
  end

  test 'order of requested scopes does not matter (mcp api == api mcp)' do
    post '/oauth/register',
         params: { redirect_uris: ['http://localhost/callback'], scope: 'mcp api' },
         as: :json
    assert_response :created

    json = JSON.parse(response.body)
    assert_equal %w[api mcp], json['scope'].split.sort
  end

  test 'extra whitespace in scope is normalized' do
    post '/oauth/register',
         params: { redirect_uris: ['http://localhost/callback'], scope: '  api   mcp  ' },
         as: :json
    assert_response :created

    json = JSON.parse(response.body)
    granted = json['scope'].split.sort
    assert_equal %w[api mcp], granted
  end

  test 'duplicate scopes are deduplicated' do
    post '/oauth/register',
         params: { redirect_uris: ['http://localhost/callback'], scope: 'api api mcp api' },
         as: :json
    assert_response :created

    json = JSON.parse(response.body)
    assert_equal %w[api mcp], json['scope'].split.sort
  end

  test 'rejects unknown scope with invalid_client_metadata' do
    assert_no_difference 'Doorkeeper::Application.count' do
      post '/oauth/register',
           params: { redirect_uris: ['http://localhost/callback'], scope: 'admin' },
           as: :json
    end

    assert_response :bad_request
    json = JSON.parse(response.body)
    assert_equal 'invalid_client_metadata', json['error']
  end

  test 'rejects request containing both valid and unknown scope tokens' do
    assert_no_difference 'Doorkeeper::Application.count' do
      post '/oauth/register',
           params: { redirect_uris: ['http://localhost/callback'], scope: 'api wallet' },
           as: :json
    end

    assert_response :bad_request
    json = JSON.parse(response.body)
    assert_equal 'invalid_client_metadata', json['error']
  end
end
