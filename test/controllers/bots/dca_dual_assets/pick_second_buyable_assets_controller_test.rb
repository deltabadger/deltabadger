require 'test_helper'

# Characterization tests for demote_to_single (the "−" button on the dual
# second-asset step), written before extracting shared wizard step base classes.
# Mirror image of promote_to_dual (covered in
# Bots::DcaSingleAssets::PickExchangesControllerTest).
class Bots::DcaDualAssets::PickSecondBuyableAssetsControllerTest < ActionDispatch::IntegrationTest
  setup do
    @user = create(:user, admin: true, setup_completed: true)
    sign_in @user
    @btc = create(:asset, :bitcoin)
    @usd = create(:asset, :usd)
    @binance = create(:binance_exchange)
    create(:ticker, :btc_usd, exchange: @binance, base_asset: @btc, quote_asset: @usd)
  end

  test 'demote_to_single moves base0 back to base_asset_id, keeps the exchange, and lands on the first incomplete single step' do
    # Single flow with exchange picked, then promoted to dual.
    get new_bots_dca_single_assets_pick_buyable_asset_path
    post bots_dca_single_assets_pick_buyable_asset_path,
         params: { bots_dca_single_asset: { base_asset_id: @btc.id } }
    post bots_dca_single_assets_pick_exchange_path,
         params: { bots_dca_single_asset: { exchange_id: @binance.id } }
    post promote_to_dual_bots_dca_single_assets_pick_exchange_path

    # Order-derived navigation now skips the already-answered exchange step:
    # with base + exchange filled (and the key valid in dry-run), the first
    # incomplete single step is :spendable.
    post demote_to_single_bots_dca_dual_assets_pick_second_buyable_asset_path
    assert_redirected_to new_bots_dca_single_assets_pick_spendable_asset_path

    assert_predicate session[:bot_config]['label'], :present?
    assert_equal @btc.id, session[:bot_config].dig('settings', 'base_asset_id')
    assert_nil session[:bot_config].dig('settings', 'base0_asset_id')
    assert_equal @binance.id.to_s, session[:bot_config]['exchange_id'].to_s
  end

  test 'new redirects to the single first step when base0 is missing' do
    get new_bots_dca_dual_assets_pick_second_buyable_asset_path
    assert_redirected_to new_bots_dca_single_assets_pick_buyable_asset_path
  end
end
