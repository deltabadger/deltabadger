require 'test_helper'

# A cold POST to a mid-wizard step (no wizard session at all — expired cookie or
# hand-crafted request) must not 500 on `session[:bot_config].merge!`/template
# nil-derefs. Mid-wizard steps turbo-redirect to root, the same contract the
# single/dual pick_spendable steps already had; the signals first step instead
# re-initialises the session and proceeds (a step-1 POST is self-sufficient,
# mirroring the single-asset first step).
class WizardSessionExpiredTest < ActionDispatch::IntegrationTest
  setup do
    @user = create(:user, admin: true, setup_completed: true)
    sign_in @user
    @btc = create(:asset, :bitcoin)
    @usd = create(:asset, :usd)
    @binance = create(:binance_exchange)
    create(:ticker, :btc_usd, exchange: @binance, base_asset: @btc, quote_asset: @usd)
  end

  def assert_turbo_redirect_to_root
    assert_response :success
    assert_match 'turbo-stream', response.body
    assert_match %(action="redirect"), response.body
  end

  test 'single pick_exchanges cold create turbo-redirects to root' do
    post bots_dca_single_assets_pick_exchange_path,
         params: { bots_dca_single_asset: { exchange_id: @binance.id } }
    assert_turbo_redirect_to_root
  end

  test 'dual pick_exchanges cold create turbo-redirects to root' do
    post bots_dca_dual_assets_pick_exchange_path,
         params: { bots_dca_dual_asset: { exchange_id: @binance.id } }
    assert_turbo_redirect_to_root
  end

  test 'dual pick_second_buyable_assets cold create turbo-redirects to root' do
    post bots_dca_dual_assets_pick_second_buyable_asset_path,
         params: { bots_dca_dual_asset: { base1_asset_id: @btc.id } }
    assert_turbo_redirect_to_root
  end

  test 'signals pick_exchanges cold create turbo-redirects to root' do
    post bots_signals_pick_exchange_path,
         params: { bots_signal: { exchange_id: @binance.id } }
    assert_turbo_redirect_to_root
  end

  test 'signals pick_spendable_assets cold create turbo-redirects to root' do
    post bots_signals_pick_spendable_asset_path,
         params: { bots_signal: { quote_asset_id: @usd.id } }
    assert_turbo_redirect_to_root
  end

  test 'index pick_spendable_assets cold create turbo-redirects to root' do
    MarketDataSettings.stubs(:current_provider).returns(MarketDataSettings::PROVIDER_DELTABADGER)
    post bots_dca_indexes_pick_spendable_asset_path,
         params: { bots_dca_index: { quote_asset_id: @usd.id } }
    assert_turbo_redirect_to_root
  end

  test 'signals pick_buyable_assets cold create re-initialises the session and proceeds' do
    post bots_signals_pick_buyable_asset_path,
         params: { bots_signal: { base_asset_id: @btc.id } }
    assert_redirected_to new_bots_signals_pick_exchange_path
    assert_equal @btc.id, session[:bot_config].dig('settings', 'base_asset_id')
  end
end
