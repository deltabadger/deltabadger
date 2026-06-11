require 'test_helper'

# Characterization tests for the signals first wizard step, written before
# extracting a shared step base class. Signals deltas vs single/dual: no
# stock-broker routing (every asset goes to the exchange step) and no
# downstream-state reset on re-pick.
class Bots::Signals::PickBuyableAssetsControllerTest < ActionDispatch::IntegrationTest
  setup do
    create(:user, admin: true)
    @user = create(:user, setup_completed: true)
    @usd = create(:asset, :usd)
    @binance = create(:binance_exchange)
    sign_in @user
  end

  test 'lists available base assets' do
    eth = create(:asset, :ethereum)
    create(:ticker, exchange: @binance, base_asset: eth, quote_asset: @usd)

    get new_bots_signals_pick_buyable_asset_path
    assert_response :ok
    assert_match 'ETH', response.body
  end

  test 'new initialises the wizard session with a label' do
    get new_bots_signals_pick_buyable_asset_path
    assert_response :ok
    assert_predicate session[:bot_config]['label'], :present?
  end

  test 'create stores the asset and routes to the exchange step' do
    btc = create(:asset, :bitcoin)
    create(:ticker, :btc_usd, exchange: @binance, base_asset: btc, quote_asset: @usd)

    get new_bots_signals_pick_buyable_asset_path
    post bots_signals_pick_buyable_asset_path,
         params: { bots_signal: { base_asset_id: btc.id } }

    assert_redirected_to new_bots_signals_pick_exchange_path
    assert_equal btc.id, session[:bot_config].dig('settings', 'base_asset_id')
  end

  test 'a stock asset also routes to the exchange step — signals have no stock-broker routing' do
    alpaca = create(:alpaca_exchange)
    aapl = create(:asset, symbol: 'AAPL', name: 'Apple Inc', category: 'Stock', external_id: 'aapl')
    create(:ticker, exchange: alpaca, base_asset: aapl, quote_asset: @usd, base: 'AAPL', quote: 'USD')

    get new_bots_signals_pick_buyable_asset_path
    post bots_signals_pick_buyable_asset_path,
         params: { bots_signal: { base_asset_id: aapl.id } }

    assert_redirected_to new_bots_signals_pick_exchange_path
  end

  # NOTE: the blank-base_asset_id branch (`render :new` from #create) is NOT
  # characterized: it 500s today because #create never sets the view ivars
  # (@bot/@assets). Unreachable from the real form, which always posts an asset id.
  # Same latent issue exists across the wizard's `render :new` failure branches.
end
