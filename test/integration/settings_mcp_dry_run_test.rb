require 'test_helper'

class SettingsMcpDryRunTest < ActionDispatch::IntegrationTest
  include Devise::Test::IntegrationHelpers

  setup do
    @admin = create(:user, admin: true, setup_completed: true)
    sign_in @admin
  end

  test 'toggle dry run on' do
    assert_not @admin.mcp_dry_run?

    patch settings_update_mcp_dry_run_path, params: { enabled: '1' }
    assert_response :success

    assert @admin.reload.mcp_dry_run?
  end

  test 'toggle dry run off' do
    @admin.mcp_dry_run = true

    patch settings_update_mcp_dry_run_path, params: { enabled: '0' }
    assert_response :success

    assert_not @admin.reload.mcp_dry_run?
  end

  test 'non-admin can toggle their own dry run' do
    regular_user = create(:user, setup_completed: true)
    sign_in regular_user

    patch settings_update_mcp_dry_run_path, params: { enabled: '1' }
    assert_response :success
    assert regular_user.reload.mcp_dry_run?
  end
end
