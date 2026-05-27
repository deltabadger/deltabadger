require 'test_helper'

class AppConfigMcpTest < ActiveSupport::TestCase
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

  test 'api_url returns the REST API v1 base under APP_ROOT_URL' do
    assert_equal 'http://localhost:3000/api/v1', AppConfig.api_url
  end

  test 'api_url honors APP_ROOT_URL override' do
    original = ENV['APP_ROOT_URL']
    ENV['APP_ROOT_URL'] = 'https://mybot.deltabadger.com'
    assert_equal 'https://mybot.deltabadger.com/api/v1', AppConfig.api_url
  ensure
    ENV['APP_ROOT_URL'] = original
  end
end
