require 'test_helper'

class Bots::DcaSingleAssets::PickExchangesControllerTest < ActionDispatch::IntegrationTest
  setup do
    @user = create(:user, admin: true, setup_completed: true)
    @asset = create(:asset)
    sign_in @user
  end

  test 'dual wizard routes are retired' do
    assert_empty Rails.application.routes.named_routes.names.grep(/dca_dual/)
  end

  test 'promote_to_multi rewrites the session into a list' do
    get new_bots_dca_single_assets_pick_buyable_asset_path
    post bots_dca_single_assets_pick_buyable_asset_path,
         params: { bots_dca_single_asset: { base_asset_id: @asset.id } }

    post promote_to_multi_bots_dca_single_assets_pick_exchange_path

    assert_redirected_to new_bots_dca_multi_assets_pick_assets_path
    assert_equal [@asset.id], session[:bot_config].dig('settings', 'base_asset_ids')
    assert_nil session[:bot_config].dig('settings', 'base_asset_id')
  end

  test 'full promoted flow: single → + → assets → exchange proceeds to api key (no loop)' do
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

    # 2. Click "+" to promote to multi
    post promote_to_multi_bots_dca_single_assets_pick_exchange_path
    assert_redirected_to new_bots_dca_multi_assets_pick_assets_path

    # 3. Add ETH and continue
    post bots_dca_multi_assets_pick_assets_path,
         params: { bots_dca_multi_asset: { base_asset_id: eth.id } }
    post advance_bots_dca_multi_assets_pick_assets_path
    assert_redirected_to new_bots_dca_multi_assets_pick_exchange_path

    # 4. Pick Binance
    post bots_dca_multi_assets_pick_exchange_path,
         params: { bots_dca_multi_asset: { exchange_id: binance.id } }
    # Should proceed to api key step, NOT loop back to the assets picker
    assert_redirected_to new_bots_dca_multi_assets_add_api_key_path
    follow_redirect!
    # In dry_run mode (tests) api_key.correct? => true, skipping to pick_spendable.
    # In real use, user enters API key then advances. Either way, NOT a loop.
    assert_not_equal new_bots_dca_multi_assets_pick_assets_path, request.path
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

  test 'exchange picker disables Hyperliquid ordering with an explanation when its trading gem is unavailable' do
    btc = create(:asset, :bitcoin)
    usd = create(:asset, :usd)
    hyperliquid = create(:hyperliquid_exchange)
    create(:ticker, :btc_usd, exchange: hyperliquid, base_asset: btc, quote_asset: usd)
    Exchanges::Hyperliquid.any_instance.stubs(:order_placement_available?).returns(false)

    get new_bots_dca_single_assets_pick_buyable_asset_path
    post bots_dca_single_assets_pick_buyable_asset_path,
         params: { bots_dca_single_asset: { base_asset_id: btc.id } }
    get new_bots_dca_single_assets_pick_exchange_path

    assert_response :success
    assert_select "button.exchange-grid__item[value='#{hyperliquid.id}'][disabled]"
    assert_select '.exchange-grid__unavailable-note', text: I18n.t('bot.hyperliquid_trading_unavailable')
  end
end
