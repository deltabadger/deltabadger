require 'test_helper'

# §8 stock-venue routing. Stock bots used to hardcode Exchanges::Alpaca.first and skip
# the exchange step entirely. This dedicated step lets a user choose a stock broker
# (Alpaca or IBKR) when more than one is available, while preserving today's zero-click
# UX when only one venue exists.
#
# The auto-select decision happens on the *stock POST* (pick_buyable#create), never on a
# GET — the wizard relies on idempotent GETs (Turbo prefetches them on hover).
class Bots::DcaSingleAssets::PickStockBrokersControllerTest < ActionDispatch::IntegrationTest
  setup do
    create(:user, admin: true) # platform requires an admin to exist before bot flows render
    @user = create(:user, setup_completed: true)
    @usd = create(:asset, :usd)
    @aapl = create(:asset, symbol: 'AAPL', name: 'Apple Inc', category: 'Stock', external_id: 'aapl')
    @alpaca = create(:alpaca_exchange)
    create(:ticker, exchange: @alpaca, base_asset: @aapl, quote_asset: @usd, base: 'AAPL', quote: 'USD')
    sign_in @user
  end

  # POST the first (stock) asset. This POST decides: auto-skip to the api-key step
  # (one venue) or route into the broker picker (2+). No state mutation happens on GET.
  def pick_aapl
    get new_bots_dca_single_assets_pick_buyable_asset_path
    post bots_dca_single_assets_pick_buyable_asset_path,
         params: { bots_dca_single_asset: { base_asset_id: @aapl.id } }
  end

  def list_ibkr_for_aapl
    ibkr = create(:ibkr_exchange)
    create(:ticker, exchange: ibkr, base_asset: @aapl, quote_asset: @usd, base: 'AAPL', quote: 'USD')
    ibkr
  end

  test 'with a single stock venue, the stock POST auto-selects it and skips to the api-key step' do
    pick_aapl
    assert_redirected_to new_bots_dca_single_assets_add_api_key_path
    assert_equal @alpaca.id.to_s, session[:bot_config]['exchange_id'].to_s
    assert_equal @usd.id, session[:bot_config].dig('settings', 'quote_asset_id')
  end

  test 'with two stock venues, the stock POST routes into the broker picker listing both' do
    ibkr = list_ibkr_for_aapl
    pick_aapl
    assert_redirected_to new_bots_dca_single_assets_pick_stock_broker_path

    get new_bots_dca_single_assets_pick_stock_broker_path
    assert_response :ok
    assert_match(/value="#{@alpaca.id}"/, response.body)
    assert_match(/value="#{ibkr.id}"/, response.body)
  end

  test 'GET new never mutates wizard state (safe against Turbo prefetch)' do
    list_ibkr_for_aapl
    pick_aapl # 2 venues -> picker, exchange not chosen yet
    refute session[:bot_config]['exchange_id'].present?

    get new_bots_dca_single_assets_pick_stock_broker_path
    assert_response :ok
    refute session[:bot_config]['exchange_id'].present?, 'GET #new must not write exchange_id'
  end

  test 'create stores the chosen broker + USD quote default and proceeds to the api-key step' do
    ibkr = list_ibkr_for_aapl
    pick_aapl
    get new_bots_dca_single_assets_pick_stock_broker_path # 2 venues -> picker

    post bots_dca_single_assets_pick_stock_broker_path,
         params: { bots_dca_single_asset: { exchange_id: ibkr.id } }

    assert_redirected_to new_bots_dca_single_assets_add_api_key_path
    assert_equal ibkr.id.to_s, session[:bot_config]['exchange_id'].to_s
    assert_equal @usd.id, session[:bot_config].dig('settings', 'quote_asset_id')
  end

  test 'create rejects an exchange that is not an available stock venue for the asset' do
    binance = create(:binance_exchange)
    list_ibkr_for_aapl
    pick_aapl
    get new_bots_dca_single_assets_pick_stock_broker_path

    post bots_dca_single_assets_pick_stock_broker_path,
         params: { bots_dca_single_asset: { exchange_id: binance.id } }

    assert_response :unprocessable_entity
    refute_equal binance.id.to_s, session[:bot_config]['exchange_id'].to_s
  end

  test 'new redirects back to pick-buyable when there is no stock asset in the session' do
    get new_bots_dca_single_assets_pick_stock_broker_path
    assert_redirected_to new_bots_dca_single_assets_pick_buyable_asset_path
  end

  test 'add_api_key#new redirects a stock bot with no chosen broker back to the broker step' do
    list_ibkr_for_aapl
    pick_aapl # 2 venues -> picker, exchange_id still blank
    get new_bots_dca_single_assets_add_api_key_path
    assert_redirected_to new_bots_dca_single_assets_pick_stock_broker_path
  end
end
