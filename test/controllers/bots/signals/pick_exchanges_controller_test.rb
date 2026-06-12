require 'test_helper'

# Characterization tests for the signals exchange step, written before extracting
# a shared step base class (the create action is textually identical across types).
class Bots::Signals::PickExchangesControllerTest < ActionDispatch::IntegrationTest
  setup do
    @user = create(:user, admin: true, setup_completed: true)
    sign_in @user
    @btc = create(:asset, :bitcoin)
    @usd = create(:asset, :usd)
    @binance = create(:binance_exchange)
    create(:ticker, :btc_usd, exchange: @binance, base_asset: @btc, quote_asset: @usd)
  end

  def seed_base_asset
    get new_bots_signals_pick_buyable_asset_path
    post bots_signals_pick_buyable_asset_path,
         params: { bots_signal: { base_asset_id: @btc.id } }
  end

  test 'new redirects to the asset step when no base asset is in the session' do
    get new_bots_signals_pick_exchange_path
    assert_redirected_to new_bots_signals_pick_buyable_asset_path
  end

  test 'new lists the exchanges trading the picked asset' do
    seed_base_asset
    get new_bots_signals_pick_exchange_path
    assert_response :ok
    assert_match(/value="#{@binance.id}"/, response.body)
  end

  test 'create merges the exchange into the session and routes to the api-key step' do
    seed_base_asset
    post bots_signals_pick_exchange_path,
         params: { bots_signal: { exchange_id: @binance.id } }

    assert_redirected_to new_bots_signals_add_api_key_path
    assert_equal @binance.id.to_s, session[:bot_config]['exchange_id'].to_s
  end

  # The blank-param 422 re-render branch is characterized in
  # wizard_create_failure_rerender_test.rb (for every wizard step).
end
