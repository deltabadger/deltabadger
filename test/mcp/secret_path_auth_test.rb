require 'test_helper'
require 'middleware/mcp_secret_path_auth'

class SecretPathAuthTest < ActiveSupport::TestCase
  setup do
    @token = AppConfig.generate_mcp_access_token!
    @inner_app = ->(env) { [200, {}, [env['PATH_INFO']]] }
    @middleware = MCPSecretPathAuth.new(@inner_app)
  end

  teardown do
    AppConfig.clear_mcp_settings!
  end

  test 'passes request with valid token in path' do
    env = Rack::MockRequest.env_for("/#{@token}")
    status, _headers, body = @middleware.call(env)

    assert_equal 200, status
    assert_equal '/', body.first
  end

  test 'passes request with valid token and trailing path' do
    env = Rack::MockRequest.env_for("/#{@token}/some/path")
    status, _headers, body = @middleware.call(env)

    assert_equal 200, status
    assert_equal '/some/path', body.first
  end

  test 'returns 404 for wrong token' do
    env = Rack::MockRequest.env_for('/wrong-token')
    status, _headers, _body = @middleware.call(env)

    assert_equal 404, status
  end

  test 'returns 404 for root path' do
    env = Rack::MockRequest.env_for('/')
    status, _headers, _body = @middleware.call(env)

    assert_equal 404, status
  end

  test 'returns 404 when MCP is not configured' do
    AppConfig.clear_mcp_settings!

    env = Rack::MockRequest.env_for('/anything')
    status, _headers, _body = @middleware.call(env)

    assert_equal 404, status
  end

  test 'sets SCRIPT_NAME to token prefix' do
    script_name_app = ->(env) { [200, {}, [env['SCRIPT_NAME']]] }
    middleware = MCPSecretPathAuth.new(script_name_app)

    env = Rack::MockRequest.env_for("/#{@token}/test")
    _status, _headers, body = middleware.call(env)

    assert_equal "/#{@token}", body.first
  end

  test 'rejects partial token match' do
    env = Rack::MockRequest.env_for("/#{@token[0..10]}")
    status, _headers, _body = @middleware.call(env)

    assert_equal 404, status
  end
end
