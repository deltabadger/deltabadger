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

  test 'with a single venue listing every stock, Next auto-selects it' do
    build_basket
    advance

    assert_redirected_to new_bots_dca_multi_assets_add_api_key_path
    assert_equal @alpaca.id.to_s, session[:bot_config]['exchange_id'].to_s
    assert_equal @usd.id, session[:bot_config].dig('settings', 'quote_asset_id')
  end

  test 'with two venues listing every stock, Next routes into the picker' do
    ibkr = list_ibkr_for_both
    build_basket
    advance

    assert_redirected_to new_bots_dca_multi_assets_pick_stock_broker_path
    get new_bots_dca_multi_assets_pick_stock_broker_path
    assert_response :ok
    assert_match(/value="#{@alpaca.id}"/, response.body)
    assert_match(/value="#{ibkr.id}"/, response.body)
  end

  test 'create stores the chosen broker and USD quote before the api-key step' do
    ibkr = list_ibkr_for_both
    build_basket
    advance

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
    advance

    post bots_dca_multi_assets_pick_stock_broker_path,
         params: { bots_dca_multi_asset: { exchange_id: binance.id } }

    assert_response :unprocessable_entity
  end

  test 'a basket whose common venue disappears returns to the asset step with an explanation' do
    build_basket
    Ticker.find_by!(exchange: @alpaca, base_asset: @msft).update!(available: false)

    advance

    assert_redirected_to new_bots_dca_single_assets_pick_buyable_asset_path
    assert_equal I18n.t('bot.dca_multi_asset.no_common_exchange'), flash[:alert]
  end

  test 'a broker chosen for one stock is kept when the basket grows and it still lists every stock' do
    list_ibkr_for_both
    pick @aapl
    advance
    post bots_dca_single_assets_pick_stock_broker_path,
         params: { bots_dca_single_asset: { exchange_id: @alpaca.id } }
    assert_equal @alpaca.id.to_s, session[:bot_config]['exchange_id'].to_s

    pick @msft
    advance

    # The key is valid in dry-run, so the next missing input is the quote — not the picker.
    assert_redirected_to new_bots_dca_multi_assets_pick_spendable_asset_path
    assert_equal @alpaca.id.to_s, session[:bot_config]['exchange_id'].to_s
  end

  test 'a chosen broker that stops listing the basket blocks Next; the exchange chip re-picks among venues carrying it' do
    ibkr = list_ibkr_for_both
    build_basket
    advance
    post bots_dca_multi_assets_pick_stock_broker_path,
         params: { bots_dca_multi_asset: { exchange_id: @alpaca.id } }
    Ticker.find_by!(exchange: @alpaca, base_asset: @msft).update!(available: false)

    get new_bots_dca_single_assets_pick_buyable_asset_path
    assert_select '.wizard-assets__warning[role="alert"]'
    advance
    assert_redirected_to new_bots_dca_single_assets_pick_buyable_asset_path
    assert_equal I18n.t('bot.dca_multi_asset.no_common_exchange'), flash[:alert]

    post advance_bots_dca_single_assets_pick_buyable_asset_path, params: { to: 'exchange' }
    assert_redirected_to new_bots_dca_multi_assets_pick_exchange_path
    follow_redirect!
    assert_select "button.exchange-grid__item[value='#{ibkr.id}']"
    assert_select "button.exchange-grid__item[value='#{@alpaca.id}']", count: 0
  end

  test 'add_api_key new redirects a stock basket with no broker back to the broker step' do
    list_ibkr_for_both
    build_basket
    advance

    get new_bots_dca_multi_assets_add_api_key_path

    assert_redirected_to new_bots_dca_multi_assets_pick_stock_broker_path
  end

  private

  def pick(asset)
    post bots_dca_single_assets_pick_buyable_asset_path,
         params: { bots_dca_single_asset: { base_asset_id: asset.id } }
  end

  def advance = post advance_bots_dca_single_assets_pick_buyable_asset_path

  def build_basket(second: @msft)
    get new_bots_dca_single_assets_pick_buyable_asset_path
    pick @aapl
    pick second
  end

  def list_ibkr_for_both
    ibkr = create(:ibkr_exchange)
    create(:ticker, exchange: ibkr, base_asset: @aapl, quote_asset: @usd, base: 'AAPL', quote: 'USD')
    create(:ticker, exchange: ibkr, base_asset: @msft, quote_asset: @usd, base: 'MSFT', quote: 'USD')
    ibkr
  end
end
