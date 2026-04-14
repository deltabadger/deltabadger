require 'test_helper'

class Bots::DcaSingleAssets::PickExchangesControllerTest < ActionDispatch::IntegrationTest
  setup do
    @user = create(:user, admin: true, setup_completed: true)
    @asset = create(:asset)
    sign_in @user
  end

  test 'promote_to_dual moves base_asset_id to base0_asset_id and redirects to dual second-asset picker' do
    # Seed session by hitting the single-asset first step.
    get new_bots_dca_single_assets_pick_buyable_asset_path
    post bots_dca_single_assets_pick_buyable_asset_path,
         params: { bots_dca_single_asset: { base_asset_id: @asset.id } }

    post promote_to_dual_bots_dca_single_assets_pick_exchange_path
    assert_redirected_to new_bots_dca_dual_assets_pick_second_buyable_asset_path

    follow_redirect!
    # The dual second-asset controller would redirect back to first if base0 wasn't set,
    # so a successful render proves base0_asset_id is now set in session.
    assert_response :success
  end

  test 'full promoted flow: single → + → second asset → exchange proceeds to api key (no loop)' do
    btc = create(:asset, :bitcoin)
    eth = create(:asset, :ethereum)
    usd = create(:asset, :usd)
    binance = create(:binance_exchange)
    create(:ticker, :btc_usd, exchange: binance, base_asset: btc, quote_asset: usd)
    create(:ticker, :eth_usd, exchange: binance, base_asset: eth, quote_asset: usd)

    get new_bots_dca_single_assets_pick_buyable_asset_path
    # 1. Pick BTC in single flow
    post bots_dca_single_assets_pick_buyable_asset_path,
         params: { bots_dca_single_asset: { base_asset_id: btc.id } }
    assert_redirected_to new_bots_dca_single_assets_pick_exchange_path

    # 2. Click "+" to promote to dual
    post promote_to_dual_bots_dca_single_assets_pick_exchange_path
    assert_redirected_to new_bots_dca_dual_assets_pick_second_buyable_asset_path

    # 3. Pick ETH as second asset
    post bots_dca_dual_assets_pick_second_buyable_asset_path,
         params: { bots_dca_dual_asset: { base1_asset_id: eth.id } }
    assert_redirected_to new_bots_dca_dual_assets_pick_exchange_path

    # 4. Pick Binance
    post bots_dca_dual_assets_pick_exchange_path,
         params: { bots_dca_dual_asset: { exchange_id: binance.id } }
    # Should proceed to api key step, NOT loop back to second-asset picker
    assert_redirected_to new_bots_dca_dual_assets_add_api_key_path
    follow_redirect!
    # In dry_run mode (tests) api_key.correct? => true, skipping to pick_spendable.
    # In real use, user enters API key then advances. Either way, NOT a loop.
    assert_not_equal new_bots_dca_dual_assets_pick_second_buyable_asset_path, request.path
    assert_not_equal new_bots_dca_single_assets_pick_buyable_asset_path, request.path
  end

  test 'single-asset flow: pick BTC then Binance proceeds to api key (no loop)' do
    btc = create(:asset, :bitcoin)
    usd = create(:asset, :usd)
    binance = create(:binance_exchange)
    create(:ticker, :btc_usd, exchange: binance, base_asset: btc, quote_asset: usd)

    get new_bots_dca_single_assets_pick_buyable_asset_path
    post bots_dca_single_assets_pick_buyable_asset_path,
         params: { bots_dca_single_asset: { base_asset_id: btc.id } }
    assert_redirected_to new_bots_dca_single_assets_pick_exchange_path

    post bots_dca_single_assets_pick_exchange_path,
         params: { bots_dca_single_asset: { exchange_id: binance.id } }
    assert_redirected_to new_bots_dca_single_assets_add_api_key_path

    follow_redirect!
    assert_not_equal new_bots_dca_single_assets_pick_buyable_asset_path, request.path
  end
end
