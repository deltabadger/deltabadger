require 'test_helper'

class AppConfigMcpDryRunTest < ActiveSupport::TestCase
  teardown do
    AppConfig.clear_mcp_settings!
  end

  test 'mcp_dry_run? returns false by default' do
    assert_not AppConfig.mcp_dry_run?
  end

  test 'mcp_dry_run? returns true when enabled' do
    AppConfig.mcp_dry_run = true
    assert AppConfig.mcp_dry_run?
  end

  test 'mcp_dry_run? returns false when disabled' do
    AppConfig.mcp_dry_run = true
    AppConfig.mcp_dry_run = false
    assert_not AppConfig.mcp_dry_run?
  end

  test 'clear_mcp_settings! clears dry run setting' do
    AppConfig.mcp_dry_run = true
    AppConfig.clear_mcp_settings!
    assert_not AppConfig.mcp_dry_run?
  end
end
