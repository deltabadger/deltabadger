require 'test_helper'

class Bots::DcaMultiAssets::PickStockBrokersControllerTest < ActionDispatch::IntegrationTest
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

  test 'with a single venue listing every stock, Continue auto-selects it' do
    build_basket
    post advance_bots_dca_multi_assets_pick_assets_path

    assert_redirected_to new_bots_dca_multi_assets_add_api_key_path
    assert_equal @alpaca.id.to_s, session[:bot_config]['exchange_id'].to_s
    assert_equal @usd.id, session[:bot_config].dig('settings', 'quote_asset_id')
  end

  test 'with two venues listing every stock, Continue routes into the picker' do
    ibkr = list_ibkr_for_both
    build_basket
    post advance_bots_dca_multi_assets_pick_assets_path

    assert_redirected_to new_bots_dca_multi_assets_pick_stock_broker_path
    get new_bots_dca_multi_assets_pick_stock_broker_path
    assert_response :ok
    assert_match(/value="#{@alpaca.id}"/, response.body)
    assert_match(/value="#{ibkr.id}"/, response.body)
  end

  test 'create stores the chosen broker and USD quote before the api-key step' do
    ibkr = list_ibkr_for_both
    build_basket
    post advance_bots_dca_multi_assets_pick_assets_path

    post bots_dca_multi_assets_pick_stock_broker_path,
         params: { bots_dca_multi_asset: { exchange_id: ibkr.id } }

    assert_redirected_to new_bots_dca_multi_assets_add_api_key_path
    assert_equal ibkr.id.to_s, session[:bot_config]['exchange_id'].to_s
    assert_equal @usd.id, session[:bot_config].dig('settings', 'quote_asset_id')
  end

  test 'create rejects a crypto exchange' do
    binance = create(:binance_exchange)
    list_ibkr_for_both
    build_basket
    post advance_bots_dca_multi_assets_pick_assets_path

    post bots_dca_multi_assets_pick_stock_broker_path,
         params: { bots_dca_multi_asset: { exchange_id: binance.id } }

    assert_response :unprocessable_entity
  end

  test 'a basket whose common venue disappears returns to assets with an explanation' do
    build_basket
    Ticker.find_by!(exchange: @alpaca, base_asset: @msft).update!(available: false)

    post advance_bots_dca_multi_assets_pick_assets_path

    assert_redirected_to new_bots_dca_multi_assets_pick_assets_path
    assert_equal I18n.t('bot.dca_multi_asset.no_common_exchange'), flash[:alert]
    refute session[:bot_config]['exchange_id'].present?
  end

  test 'a broker chosen for the single bot is cleared when the basket has multiple venues' do
    list_ibkr_for_both
    get new_bots_dca_single_assets_pick_buyable_asset_path
    post bots_dca_single_assets_pick_buyable_asset_path,
         params: { bots_dca_single_asset: { base_asset_id: @aapl.id } }
    post bots_dca_single_assets_pick_stock_broker_path,
         params: { bots_dca_single_asset: { exchange_id: @alpaca.id } }
    assert_equal @alpaca.id.to_s, session[:bot_config]['exchange_id'].to_s

    post promote_to_multi_bots_dca_single_assets_pick_exchange_path
    post bots_dca_multi_assets_pick_assets_path,
         params: { bots_dca_multi_asset: { base_asset_id: @msft.id } }
    post advance_bots_dca_multi_assets_pick_assets_path

    assert_redirected_to new_bots_dca_multi_assets_pick_stock_broker_path
    refute session[:bot_config]['exchange_id'].present?
  end

  test 'add_api_key new redirects a stock basket with no broker back to the broker step' do
    list_ibkr_for_both
    build_basket
    post advance_bots_dca_multi_assets_pick_assets_path

    get new_bots_dca_multi_assets_add_api_key_path

    assert_redirected_to new_bots_dca_multi_assets_pick_stock_broker_path
  end

  private

  def build_basket(second: @msft)
    get new_bots_dca_single_assets_pick_buyable_asset_path
    post bots_dca_single_assets_pick_buyable_asset_path,
         params: { bots_dca_single_asset: { base_asset_id: @aapl.id } }
    post promote_to_multi_bots_dca_single_assets_pick_exchange_path
    post bots_dca_multi_assets_pick_assets_path,
         params: { bots_dca_multi_asset: { base_asset_id: second.id } }
  end

  def list_ibkr_for_both
    ibkr = create(:ibkr_exchange)
    create(:ticker, exchange: ibkr, base_asset: @aapl, quote_asset: @usd, base: 'AAPL', quote: 'USD')
    create(:ticker, exchange: ibkr, base_asset: @msft, quote_asset: @usd, base: 'MSFT', quote: 'USD')
    ibkr
  end
end
