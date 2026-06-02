require 'test_helper'

class Bots::NewPickerTest < ActionDispatch::IntegrationTest
  # admin + setup_completed mirror tracker_test.rb: without an admin the global setup
  # gate redirects away from /bots/new and would mask the picker assertions.
  def sign_in_user(advanced:)
    user = create(:user, admin: true, setup_completed: true, advanced_bots_enabled: advanced)
    sign_in user
    user
  end

  test 'picker always offers the basic bots (DCA + Index) without redirecting' do
    sign_in_user(advanced: false)

    get new_bot_path
    assert_response :success
    assert_select "a[href='#{new_bots_dca_single_assets_pick_buyable_asset_path}']"
    assert_select "a[href='#{new_bots_dca_indexes_setup_coingecko_path}']"
  end

  test 'Signal bot is hidden when advanced bots are disabled' do
    sign_in_user(advanced: false)

    get new_bot_path
    assert_response :success
    assert_select "a[href='#{new_bots_signals_pick_buyable_asset_path}']", count: 0
  end

  test 'all three bot types are shown when advanced bots are enabled' do
    sign_in_user(advanced: true)

    get new_bot_path
    assert_response :success
    assert_select "a[href='#{new_bots_dca_single_assets_pick_buyable_asset_path}']"
    assert_select "a[href='#{new_bots_dca_indexes_setup_coingecko_path}']"
    assert_select "a[href='#{new_bots_signals_pick_buyable_asset_path}']"
  end
end
