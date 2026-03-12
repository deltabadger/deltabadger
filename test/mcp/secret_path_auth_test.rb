# frozen_string_literal: true

require 'test_helper'
require 'middleware/mcp_secret_path_auth'

class SecretPathAuthTest < ActiveSupport::TestCase
  setup do
    @token = AppConfig.generate_mcp_access_token!
    @rails_app = ->(env) { [200, {}, ["rails:#{env['PATH_INFO']}"]] }
    @middleware = MCPSecretPathAuth.new(@rails_app)
    # Stub ActionMCP.server to return a simple Rack app
    @mcp_app = ->(env) { [200, {}, ["mcp:#{env['PATH_INFO']}"]] }
    ActionMCP.stubs(:server).returns(@mcp_app)
  end

  teardown do
    AppConfig.clear_mcp_settings!
  end

  test 'routes request with valid token to MCP server' do
    env = Rack::MockRequest.env_for("/#{@token}")
    status, _headers, body = @middleware.call(env)

    assert_equal 200, status
    assert_equal 'mcp:/', body.first
  end

  test 'routes request with valid token and trailing path to MCP server' do
    env = Rack::MockRequest.env_for("/#{@token}/some/path")
    status, _headers, body = @middleware.call(env)

    assert_equal 200, status
    assert_equal 'mcp:/some/path', body.first
  end

  test 'passes non-token requests through to Rails' do
    env = Rack::MockRequest.env_for('/wrong-token')
    _status, _headers, body = @middleware.call(env)

    assert_equal 'rails:/wrong-token', body.first
  end

  test 'passes root path through to Rails' do
    env = Rack::MockRequest.env_for('/')
    _status, _headers, body = @middleware.call(env)

    assert_equal 'rails:/', body.first
  end

  test 'passes through to Rails when MCP is not configured' do
    AppConfig.clear_mcp_settings!

    env = Rack::MockRequest.env_for('/anything')
    _status, _headers, body = @middleware.call(env)

    assert_equal 'rails:/anything', body.first
  end

  test 'sets SCRIPT_NAME to token prefix' do
    script_name_app = ->(env) { [200, {}, [env['SCRIPT_NAME']]] }
    ActionMCP.stubs(:server).returns(script_name_app)

    env = Rack::MockRequest.env_for("/#{@token}/test")
    _status, _headers, body = @middleware.call(env)

    assert_equal "/#{@token}", body.first
  end

  test 'rejects partial token match and passes to Rails' do
    env = Rack::MockRequest.env_for("/#{@token[0..10]}")
    _status, _headers, body = @middleware.call(env)

    assert_equal "rails:/#{@token[0..10]}", body.first
  end
end
