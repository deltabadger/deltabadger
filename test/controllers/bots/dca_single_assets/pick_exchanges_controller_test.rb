require 'test_helper'

class Bots::DcaSingleAssets::PickExchangesControllerTest < ActionDispatch::IntegrationTest
  setup do
    @user = create(:user, admin: true, setup_completed: true)
    sign_in @user
    # Asset-first is the optional route, but it is where the basket mechanics under test are
    # easiest to drive (and where stock routing lives), so these tests opt into it.
    post bots_dca_single_assets_order_path, params: { flow: 'asset_first' }
  end

  test 'dual wizard routes are retired' do
    assert_empty Rails.application.routes.named_routes.names.grep(/dca_dual/)
  end

  test 'the promote-to-multi hop and the separate multi picker are gone' do
    assert_empty Rails.application.routes.named_routes.names
                      .grep(/promote_to_multi|dca_multi_assets_pick_assets|dca_multi_assets_order/)
  end

  test 'full basket flow: pick → pick → Next → exchange proceeds to api key (no loop)' do
    btc = create(:asset, :bitcoin)
    eth = create(:asset, :ethereum)
    usd = create(:asset, :usd)
    binance = create(:binance_exchange)
    create(:ticker, :btc_usd, exchange: binance, base_asset: btc, quote_asset: usd)
    create(:ticker, :eth_usd, exchange: binance, base_asset: eth, quote_asset: usd)

    get new_bots_dca_single_assets_pick_buyable_asset_path
    post bots_dca_single_assets_pick_buyable_asset_path,
         params: { bots_dca_single_asset: { base_asset_id: btc.id } }
    post bots_dca_single_assets_pick_buyable_asset_path,
         params: { bots_dca_single_asset: { base_asset_id: eth.id } }
    post advance_bots_dca_single_assets_pick_buyable_asset_path
    assert_redirected_to new_bots_dca_multi_assets_pick_exchange_path

    post bots_dca_multi_assets_pick_exchange_path,
         params: { bots_dca_multi_asset: { exchange_id: binance.id } }
    # Should proceed to api key step, NOT loop back to the assets picker
    assert_redirected_to new_bots_dca_multi_assets_add_api_key_path
    follow_redirect!
    # In dry_run mode (tests) api_key.correct? => true, skipping to pick_spendable.
    assert_not_equal new_bots_dca_single_assets_pick_buyable_asset_path, request.path
  end

  test 'one asset: pick BTC, Next, then Binance proceeds to the basket api key (no loop)' do
    btc = create(:asset, :bitcoin)
    usd = create(:asset, :usd)
    binance = create(:binance_exchange)
    create(:ticker, :btc_usd, exchange: binance, base_asset: btc, quote_asset: usd)

    get new_bots_dca_single_assets_pick_buyable_asset_path
    post bots_dca_single_assets_pick_buyable_asset_path,
         params: { bots_dca_single_asset: { base_asset_id: btc.id } }
    assert_redirected_to new_bots_dca_single_assets_pick_buyable_asset_path
    post advance_bots_dca_single_assets_pick_buyable_asset_path
    assert_redirected_to new_bots_dca_multi_assets_pick_exchange_path

    post bots_dca_multi_assets_pick_exchange_path,
         params: { bots_dca_multi_asset: { exchange_id: binance.id } }
    assert_redirected_to new_bots_dca_multi_assets_add_api_key_path

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
    post advance_bots_dca_single_assets_pick_buyable_asset_path
    get new_bots_dca_multi_assets_pick_exchange_path

    assert_response :success
    assert_select "button.exchange-grid__item[value='#{hyperliquid.id}'][disabled]"
    assert_select '.exchange-grid__unavailable-note', text: I18n.t('bot.hyperliquid_trading_unavailable')
  end
end

# A wizard the previous release left open keeps its one asset in settings.base_asset_id. Re-picking the
# exchange keeps it, as it always did, until the asset step rewrites it as a list.
class Bots::DcaSingleAssets::PickExchangesLegacySessionTest < ActionController::TestCase
  tests Bots::DcaSingleAssets::PickExchangesController
  include Devise::Test::ControllerHelpers

  test 're-picking the exchange keeps a one-asset selection from the previous release' do
    sign_in create(:user, admin: true, setup_completed: true)
    btc = create(:asset, :bitcoin)
    usd = create(:asset, :usd)
    binance = create(:binance_exchange)
    kraken = create(:kraken_exchange)
    [binance, kraken].each { |venue| create(:ticker, exchange: venue, base_asset: btc, quote_asset: usd) }

    post :create, params: { bots_dca_single_asset: { exchange_id: kraken.id } },
                  session: { bot_config: { 'flow' => 'exchange_first', 'exchange_id' => binance.id,
                                           'settings' => { 'base_asset_id' => btc.id } } }

    assert_equal kraken.id.to_s, session[:bot_config]['exchange_id'].to_s
    assert_equal btc.id, session[:bot_config].dig('settings', 'base_asset_id')
  end
end
