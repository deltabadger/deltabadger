require 'test_helper'
require 'tmpdir'

class VersionTest < ActiveSupport::TestCase
  test 'warns when Cargo.toml is absent and the version falls back' do
    initializer = Rails.root.join('config/initializers/version.rb')
    original_version = Rails.application.config.version
    original_mcp_version = Rails.application.config.action_mcp.version

    Dir.mktmpdir do |root|
      cargo_toml_path = Pathname(root).join('src-tauri/Cargo.toml')
      Rails.stubs(:root).returns(Pathname(root))
      Rails.logger.expects(:warn).with(
        "Application version unavailable: #{cargo_toml_path} does not exist; using 0.0.0"
      )

      load initializer

      assert_equal '0.0.0', Rails.application.config.version
      assert_equal '0.0.0', Rails.application.config.action_mcp.version
    end
  ensure
    Rails.application.config.version = original_version
    Rails.application.config.action_mcp.version = original_mcp_version
  end
end
