require 'test_helper'

class AppConfigMcpTest < ActiveSupport::TestCase
  teardown do
    AppConfig.clear_mcp_settings!
  end

  test 'mcp_configured? returns true when MCP_ENABLED is true' do
    original = ENV['MCP_ENABLED']
    ENV['MCP_ENABLED'] = 'true'
    assert AppConfig.mcp_configured?
  ensure
    ENV['MCP_ENABLED'] = original
  end

  test 'mcp_configured? returns false when MCP_ENABLED is not set' do
    original = ENV['MCP_ENABLED']
    ENV['MCP_ENABLED'] = nil
    assert_not AppConfig.mcp_configured?
  ensure
    ENV['MCP_ENABLED'] = original
  end

  test 'mcp_url returns fixed /mcp path' do
    url = AppConfig.mcp_url
    assert_equal 'http://localhost:3000/mcp', url
  end

  test 'mcp_url uses APP_ROOT_URL' do
    original = ENV['APP_ROOT_URL']
    ENV['APP_ROOT_URL'] = 'https://mybot.deltabadger.com'
    url = AppConfig.mcp_url
    assert_equal 'https://mybot.deltabadger.com/mcp', url
  ensure
    ENV['APP_ROOT_URL'] = original
  end

  test 'clear_mcp_settings! removes tool permissions and dry run' do
    AppConfig.set_mcp_tool_enabled('start_bot', true)
    AppConfig.mcp_dry_run = true

    AppConfig.clear_mcp_settings!

    assert_not AppConfig.mcp_dry_run?
    assert_not AppConfig.mcp_tool_enabled?('start_bot')
  end
end
