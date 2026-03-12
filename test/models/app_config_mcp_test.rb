require 'test_helper'

class AppConfigMcpTest < ActiveSupport::TestCase
  teardown do
    AppConfig.clear_mcp_settings!
  end

  test 'generate_mcp_access_token! creates a 64-char hex token' do
    token = AppConfig.generate_mcp_access_token!
    assert_match(/\A[a-f0-9]{32}\z/, token)
  end

  test 'mcp_access_token returns the stored token' do
    AppConfig.generate_mcp_access_token!
    assert_equal AppConfig.get('mcp_access_token'), AppConfig.mcp_access_token
  end

  test 'mcp_access_token returns nil when not set' do
    assert_nil AppConfig.mcp_access_token
  end

  test 'mcp_configured? returns true when token exists' do
    AppConfig.generate_mcp_access_token!
    assert AppConfig.mcp_configured?
  end

  test 'mcp_configured? returns false when token not set' do
    assert_not AppConfig.mcp_configured?
  end

  test 'clear_mcp_settings! removes the token' do
    AppConfig.generate_mcp_access_token!
    assert AppConfig.mcp_configured?

    AppConfig.clear_mcp_settings!
    assert_not AppConfig.mcp_configured?
  end

  test 'generate_mcp_access_token! overwrites existing token' do
    first_token = AppConfig.generate_mcp_access_token!
    second_token = AppConfig.generate_mcp_access_token!

    assert_not_equal first_token, second_token
    assert_equal second_token, AppConfig.mcp_access_token
  end

  test 'mcp_url includes token in path' do
    token = AppConfig.generate_mcp_access_token!
    url = AppConfig.mcp_url

    assert_match(%r{:3001/#{token}\z}, url)
  end

  test 'mcp_url returns nil when not configured' do
    assert_nil AppConfig.mcp_url
  end

  test 'mcp_url uses MCP_PORT env var' do
    token = AppConfig.generate_mcp_access_token!

    original = ENV['MCP_PORT']
    ENV['MCP_PORT'] = '4000'
    url = AppConfig.mcp_url
    assert_match(%r{:4000/#{token}\z}, url)
  ensure
    ENV['MCP_PORT'] = original
  end
end
