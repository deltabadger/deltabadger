# frozen_string_literal: true

require 'test_helper'

class AppConfigToolsTest < ActiveSupport::TestCase
  test 'REST exposes every MCP tool, all off by default' do
    assert_equal AppConfig::MCP_TOOL_DEFAULTS.keys.sort, AppConfig::REST_TOOL_DEFAULTS.keys.sort
    assert AppConfig::REST_TOOL_DEFAULTS.values.none?
  end

  test 'REST groups tools exactly as MCP does' do
    assert_equal AppConfig::MCP_TOOL_GROUPS, AppConfig::REST_TOOL_GROUPS
  end

  test 'every tool class on disk is registered, and every registered tool has a class' do
    on_disk = Dir[Rails.root.join('app/mcp/tools/*_tool.rb')]
              .map { |path| File.basename(path, '_tool.rb') } - ['application_mcp']
    assert_equal on_disk.sort, AppConfig::MCP_TOOL_DEFAULTS.keys.sort
  end

  test 'every registered tool belongs to exactly one group' do
    assert_equal AppConfig::MCP_TOOL_DEFAULTS.keys.sort, AppConfig::TOOL_GROUPS.values.flatten.sort
    assert_equal AppConfig::TOOL_GROUPS.values.flatten.size, AppConfig::TOOL_GROUPS.values.flatten.uniq.size
  end

  # fallback: false, or the English string satisfies every locale (config.i18n.fallbacks = true).
  test 'every tool has a native label and description in every locale' do
    I18n.available_locales.each do |locale|
      AppConfig::MCP_TOOL_DEFAULTS.each_key do |tool|
        %w[label description].each do |leaf|
          key = "settings.mcp.tools.#{tool}.#{leaf}"
          assert I18n.exists?(key, locale, fallback: false), "#{locale}: #{key} is missing"
        end
      end
    end
  end
end
