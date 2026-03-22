# frozen_string_literal: true

require 'test_helper'

class UserMcpDryRunTest < ActiveSupport::TestCase
  setup do
    @user = create(:user, admin: true)
  end

  test 'mcp_dry_run? returns false by default' do
    assert_not @user.mcp_dry_run?
  end

  test 'mcp_dry_run? returns true when enabled' do
    @user.mcp_dry_run = true
    assert @user.reload.mcp_dry_run?
  end

  test 'mcp_dry_run? returns false when disabled' do
    @user.mcp_dry_run = true
    @user.mcp_dry_run = false
    assert_not @user.reload.mcp_dry_run?
  end
end
