require 'test_helper'

# §8 stock-venue routing for dual-asset bots. Mirrors the single-asset step: the
# second-asset POST auto-selects the only venue listing both stocks, or routes into a
# broker picker when 2+ venues qualify. A mixed stock+crypto pair has no shared stock
# venue and is treated as unsupported (the user is sent back to re-pick the second asset).
class Bots::DcaDualAssets::PickStockBrokersControllerTest < ActionDispatch::IntegrationTest
  setup do
    create(:user, admin: true)
    @user = create(:user, setup_completed: true)
    @usd = create(:asset, :usd)
    @aapl = create(:asset, symbol: 'AAPL', name: 'Apple Inc', category: 'Stock', external_id: 'aapl')
    @msft = create(:asset, symbol: 'MSFT', name: 'Microsoft', category: 'Stock', external_id: 'msft')
    @alpaca = create(:alpaca_exchange)
    create(:ticker, exchange: @alpaca, base_asset: @aapl, quote_asset: @usd, base: 'AAPL', quote: 'USD')
    create(:ticker, exchange: @alpaca, base_asset: @msft, quote_asset: @usd, base: 'MSFT', quote: 'USD')
    sign_in @user
  end

  # base0 = AAPL (promoted from single), base1 = the given second asset. The second-asset
  # POST decides auto-skip vs picker, same as the single-asset flow.
  def pick_dual(second: @msft)
    get new_bots_dca_single_assets_pick_buyable_asset_path
    post bots_dca_single_assets_pick_buyable_asset_path,
         params: { bots_dca_single_asset: { base_asset_id: @aapl.id } }
    post promote_to_dual_bots_dca_single_assets_pick_exchange_path
    post bots_dca_dual_assets_pick_second_buyable_asset_path,
         params: { bots_dca_dual_asset: { base1_asset_id: second.id } }
  end

  def list_ibkr_for_both
    ibkr = create(:ibkr_exchange)
    create(:ticker, exchange: ibkr, base_asset: @aapl, quote_asset: @usd, base: 'AAPL', quote: 'USD')
    create(:ticker, exchange: ibkr, base_asset: @msft, quote_asset: @usd, base: 'MSFT', quote: 'USD')
    ibkr
  end

  test 'with a single venue listing both stocks, the second-asset POST auto-selects it' do
    pick_dual
    assert_redirected_to new_bots_dca_dual_assets_add_api_key_path
    assert_equal @alpaca.id.to_s, session[:bot_config]['exchange_id'].to_s
    assert_equal @usd.id, session[:bot_config].dig('settings', 'quote_asset_id')
  end

  test 'with two venues listing both stocks, the second-asset POST routes into the picker' do
    ibkr = list_ibkr_for_both
    pick_dual
    assert_redirected_to new_bots_dca_dual_assets_pick_stock_broker_path

    get new_bots_dca_dual_assets_pick_stock_broker_path
    assert_response :ok
    assert_match(/value="#{@alpaca.id}"/, response.body)
    assert_match(/value="#{ibkr.id}"/, response.body)
  end

  test 'create stores the chosen broker + USD quote default and proceeds to the api-key step' do
    ibkr = list_ibkr_for_both
    pick_dual
    get new_bots_dca_dual_assets_pick_stock_broker_path

    post bots_dca_dual_assets_pick_stock_broker_path,
         params: { bots_dca_dual_asset: { exchange_id: ibkr.id } }

    assert_redirected_to new_bots_dca_dual_assets_add_api_key_path
    assert_equal ibkr.id.to_s, session[:bot_config]['exchange_id'].to_s
    assert_equal @usd.id, session[:bot_config].dig('settings', 'quote_asset_id')
  end

  test 'create rejects a crypto exchange' do
    binance = create(:binance_exchange)
    list_ibkr_for_both
    pick_dual
    get new_bots_dca_dual_assets_pick_stock_broker_path

    post bots_dca_dual_assets_pick_stock_broker_path,
         params: { bots_dca_dual_asset: { exchange_id: binance.id } }

    assert_response :unprocessable_entity
  end

  test 'a mixed stock+crypto pair has no shared stock venue and returns to second-asset pick' do
    btc = create(:asset, :bitcoin)
    binance = create(:binance_exchange)
    create(:ticker, :btc_usd, exchange: binance, base_asset: btc, quote_asset: @usd)

    pick_dual(second: btc) # base0 = AAPL (Alpaca), base1 = BTC (Binance): no shared stock venue
    assert_redirected_to new_bots_dca_dual_assets_pick_second_buyable_asset_path
    refute session[:bot_config]['exchange_id'].present?
  end

  test 'a broker chosen for the single bot is cleared when promoting to a multi-venue dual pair' do
    list_ibkr_for_both # AAPL + MSFT on both Alpaca and IBKR

    # Single AAPL bot has two venues -> the user explicitly picks Alpaca.
    get new_bots_dca_single_assets_pick_buyable_asset_path
    post bots_dca_single_assets_pick_buyable_asset_path,
         params: { bots_dca_single_asset: { base_asset_id: @aapl.id } }
    post bots_dca_single_assets_pick_stock_broker_path,
         params: { bots_dca_single_asset: { exchange_id: @alpaca.id } }
    assert_equal @alpaca.id.to_s, session[:bot_config]['exchange_id'].to_s

    # Promote to dual and add a second stock that is also on two venues: the prior Alpaca
    # choice must be dropped so the user is forced to choose again.
    post promote_to_dual_bots_dca_single_assets_pick_exchange_path
    post bots_dca_dual_assets_pick_second_buyable_asset_path,
         params: { bots_dca_dual_asset: { base1_asset_id: @msft.id } }

    assert_redirected_to new_bots_dca_dual_assets_pick_stock_broker_path
    refute session[:bot_config]['exchange_id'].present?, 'stale single-bot broker must be cleared'
  end

  test 'add_api_key#new redirects a dual stock bot with no chosen broker back to the broker step' do
    list_ibkr_for_both
    pick_dual # 2 venues -> picker, exchange_id blank
    get new_bots_dca_dual_assets_add_api_key_path
    assert_redirected_to new_bots_dca_dual_assets_pick_stock_broker_path
  end
end
