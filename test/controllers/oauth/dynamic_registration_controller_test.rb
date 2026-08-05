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

  # ---- client metadata bounds ---------------------------------------------

  test 'truncates an oversized client_name' do
    post '/oauth/register', params: {
      redirect_uris: ['https://example.test/cb'],
      client_name: 'A' * 5_000
    }, as: :json

    assert_response :created
    assert_equal Oauth::DynamicRegistrationController::MAX_CLIENT_NAME_LENGTH,
                 Doorkeeper::Application.last.name.length
  end

  # Doorkeeper's own validator ALREADY rejects javascript:, data:, relative URIs and
  # fragments — verified against the pinned gem. Testing those would prove nothing about
  # our change. What it ACCEPTS today, and what the http(s) allowlist actually closes,
  # is custom/native schemes: myapp://cb, com.evil.app:/cb, urn:ietf:wg:oauth:2.0:oob.
  test 'rejects a custom-scheme redirect_uri that Doorkeeper would otherwise accept' do
    post '/oauth/register', params: {
      redirect_uris: ['myapp://cb'], client_name: 'X'
    }, as: :json

    assert_response :bad_request
    assert_equal 'invalid_redirect_uri', response.parsed_body['error']
  end

  test 'rejects an oob redirect_uri' do
    post '/oauth/register', params: {
      redirect_uris: ['urn:ietf:wg:oauth:2.0:oob'], client_name: 'X'
    }, as: :json

    assert_response :bad_request
  end

  # javascript: has scheme "javascript", not http(s), so absolute_http_uri? rejects it
  # before Doorkeeper's own validator (and create!) is ever reached.
  test 'a javascript redirect_uri is rejected as invalid_redirect_uri' do
    post '/oauth/register', params: {
      redirect_uris: ['javascript:alert(1)'], client_name: 'X'
    }, as: :json

    assert_response :bad_request
    assert_equal 'invalid_redirect_uri', response.parsed_body['error']
  end

  # A fragment passes our own absolute_http_uri? check (scheme/host are fine), so this
  # is the one case that reaches create! and depends on the RecordInvalid rescue: without
  # it, Doorkeeper's own validator rejection would surface as an unhandled 422, not a
  # clean 400.
  test 'a fragment in the redirect_uri is a clean 400 via the RecordInvalid rescue' do
    post '/oauth/register', params: {
      redirect_uris: ['https://example.test/cb#x'], client_name: 'X'
    }, as: :json

    assert_response :bad_request
    assert_equal 'invalid_redirect_uri', response.parsed_body['error']
  end

  test 'caps the number of redirect_uris' do
    post '/oauth/register', params: {
      redirect_uris: Array.new(30) { |i| "https://example.test/cb#{i}" }, client_name: 'X'
    }, as: :json

    assert_response :bad_request
  end

  test 'caps the length of a single redirect_uri' do
    post '/oauth/register', params: {
      redirect_uris: ["https://example.test/#{'a' * 5_000}"], client_name: 'X'
    }, as: :json

    assert_response :bad_request
  end

  test 'still accepts a well-formed registration' do
    assert_difference -> { Doorkeeper::Application.count }, 1 do
      post '/oauth/register', params: {
        redirect_uris: ['https://example.test/cb'], client_name: 'Claude', scope: 'mcp'
      }, as: :json
    end
    assert_response :created
  end
end
