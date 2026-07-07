require 'test_helper'

# When no stock catalog is present (self-hosted before the admin's Alpaca
# bootstrap sync), the asset picker tells non-admins to ask their admin and
# gives admins a CTA to the Settings activation. Hosted containers always have
# a catalog (data API), and a previously synced catalog stays active even if
# the credential is later removed, so the notice never renders in either case.
class StockActivationNoticeTest < ActionDispatch::IntegrationTest
  include Devise::Test::IntegrationHelpers

  setup do
    @admin = create(:user, admin: true, setup_completed: true)
  end

  test 'non-admin sees the ask-admin notice when no stock source is active' do
    sign_in create(:user, setup_completed: true)

    get new_bots_dca_single_assets_pick_buyable_asset_path

    assert_response :success
    assert_match I18n.t('bot.setup.stocks_ask_admin'), response.body
    refute_match I18n.t('bot.setup.stocks_activate_cta'), response.body
  end

  test 'admin sees an activation CTA linking to Settings' do
    sign_in @admin

    get new_bots_dca_single_assets_pick_buyable_asset_path

    assert_response :success
    assert_match I18n.t('bot.setup.stocks_activate_cta'), response.body
    assert_match settings_connect_path, response.body
  end

  test 'notice is absent when a synced stock catalog exists, even with no credential' do
    # Post-disconnect state: catalog synced earlier, credential since removed.
    exchange = create(:alpaca_exchange)
    aapl = create(:asset, external_id: 'alpaca_uuid-aapl', symbol: 'AAPL', name: 'Apple Inc', category: 'Stock')
    usd = create(:asset, :usd)
    create(:ticker, exchange: exchange, base_asset: aapl, quote_asset: usd, available: true)
    sign_in create(:user, setup_completed: true)

    get new_bots_dca_single_assets_pick_buyable_asset_path

    refute_match I18n.t('bot.setup.stocks_ask_admin'), response.body
  end

  test 'notice is absent on hosted' do
    MarketDataSettings.stubs(:current_provider).returns(MarketDataSettings::PROVIDER_DELTABADGER)
    sign_in create(:user, setup_completed: true)

    get new_bots_dca_single_assets_pick_buyable_asset_path

    refute_match I18n.t('bot.setup.stocks_ask_admin'), response.body
  end

  test 'welcome screen adapts the stocks line to activation state and role' do
    # Renders only for users with no bots (bots/index.html.erb) — fresh users qualify.
    # Inactive + non-admin: ask your admin, no Settings link.
    sign_in create(:user, setup_completed: true)
    get bots_path
    assert_match I18n.t('bot.setup.stocks_ask_admin'), response.body
    assert_select ".welcome-screen a[href='#{settings_connect_path}']", count: 0

    # Inactive + admin: the existing connect-Alpaca-in-Settings line.
    sign_in @admin
    get bots_path
    assert_select ".welcome-screen a[href='#{settings_connect_path}']", count: 1

    # Active: no stocks onboarding line at all. A synced catalog is what
    # activates — not the credential.
    exchange = create(:alpaca_exchange)
    aapl = create(:asset, external_id: 'alpaca_uuid-aapl', symbol: 'AAPL', name: 'Apple Inc', category: 'Stock')
    usd = create(:asset, :usd)
    create(:ticker, exchange: exchange, base_asset: aapl, quote_asset: usd, available: true)
    get bots_path
    refute_match I18n.t('bot.setup.stocks_ask_admin'), response.body
    assert_select ".welcome-screen a[href='#{settings_connect_path}']", count: 0
  end

  test 'notice renders in the dual second-asset and signal pickers too' do
    sign_in create(:user, setup_completed: true)
    btc = create(:asset, :bitcoin)
    usd = create(:asset, :usd)
    binance = create(:binance_exchange)
    create(:ticker, :btc_usd, exchange: binance, base_asset: btc, quote_asset: usd)

    # Dual: reach the second-asset step via the single flow promote.
    get new_bots_dca_single_assets_pick_buyable_asset_path
    post bots_dca_single_assets_pick_buyable_asset_path,
         params: { bots_dca_single_asset: { base_asset_id: btc.id } }
    post promote_to_dual_bots_dca_single_assets_pick_exchange_path
    get new_bots_dca_dual_assets_pick_second_buyable_asset_path
    assert_match I18n.t('bot.setup.stocks_ask_admin'), response.body

    get new_bots_signals_pick_buyable_asset_path
    assert_match I18n.t('bot.setup.stocks_ask_admin'), response.body
  end
end
