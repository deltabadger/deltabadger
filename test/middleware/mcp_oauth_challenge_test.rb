# frozen_string_literal: true

require 'test_helper'
require 'middleware/mcp_oauth_challenge'

class McpOauthChallengeTest < ActiveSupport::TestCase
  setup do
    @app = ->(env) { [200, {}, ["ok:#{env['PATH_INFO']}"]] }
    @middleware = McpOauthChallenge.new(@app)
  end

  test 'returns 401 with WWW-Authenticate for unauthenticated /mcp request' do
    env = Rack::MockRequest.env_for('/mcp', 'HTTP_HOST' => 'example.com')
    status, headers, _body = @middleware.call(env)

    assert_equal 401, status
    assert_match %r{resource_metadata="http://example.com/\.well-known/oauth-protected-resource"}, headers['WWW-Authenticate']
  end

  test 'passes through when bearer token is present' do
    env = Rack::MockRequest.env_for('/mcp', 'HTTP_HOST' => 'example.com', 'HTTP_AUTHORIZATION' => 'Bearer abc123')
    status, _headers, body = @middleware.call(env)

    assert_equal 200, status
    assert_equal 'ok:/mcp', body.first
  end

  test 'passes through non-mcp requests' do
    env = Rack::MockRequest.env_for('/settings', 'HTTP_HOST' => 'example.com')
    status, _headers, body = @middleware.call(env)

    assert_equal 200, status
    assert_equal 'ok:/settings', body.first
  end

  test 'challenges /mcp subpaths too' do
    env = Rack::MockRequest.env_for('/mcp/sse', 'HTTP_HOST' => 'example.com')
    status, headers, _body = @middleware.call(env)

    assert_equal 401, status
    assert headers['WWW-Authenticate'].present?
  end
end
