require 'test_helper'

# Characterization tests for the pick-spendable-asset wizard step, written before
# extracting a shared step base class. This step finalises single/dual/index bots
# (defaults + save in :created state, NOT started) and, for signals, only merges the
# quote asset and hands off to confirm_settings. The wizard session is seeded through
# real requests (same pattern as Bots::DcaSingleAssets::PickExchangesControllerTest).
class Bots::DcaSingleAssets::PickSpendableAssetsControllerTest < ActionDispatch::IntegrationTest
  setup do
    @user = create(:user, admin: true, setup_completed: true)
    sign_in @user
    @btc = create(:asset, :bitcoin)
    @usd = create(:asset, :usd)
    @binance = create(:binance_exchange)
    create(:ticker, :btc_usd, exchange: @binance, base_asset: @btc, quote_asset: @usd)
  end

  def seed_wizard_session
    get new_bots_dca_single_assets_pick_buyable_asset_path
    post bots_dca_single_assets_pick_buyable_asset_path,
         params: { bots_dca_single_asset: { base_asset_id: @btc.id } }
    post bots_dca_single_assets_pick_exchange_path,
         params: { bots_dca_single_asset: { exchange_id: @binance.id } }
  end

  test 'new renders the quote asset list' do
    seed_wizard_session
    get new_bots_dca_single_assets_pick_spendable_asset_path
    assert_response :success
    assert_match 'USD', response.body
  end

  test 'new redirects to the api-key step when the key is not correct' do
    seed_wizard_session
    ApiKey.any_instance.stubs(:correct?).returns(false)
    get new_bots_dca_single_assets_pick_spendable_asset_path
    assert_redirected_to new_bots_dca_single_assets_add_api_key_path
  end

  test 'create finalises an unstarted bot with default quote_amount 100 and weekly interval' do
    seed_wizard_session
    assert_difference -> { @user.bots.count }, 1 do
      post bots_dca_single_assets_pick_spendable_asset_path,
           params: { bots_dca_single_asset: { quote_asset_id: @usd.id } }
    end
    assert_response :success

    bot = @user.bots.order(:id).last
    assert_instance_of Bots::DcaSingleAsset, bot
    assert_predicate bot, :created?, 'finalise must NOT start the bot'
    assert_equal 100, bot.quote_amount
    assert_equal 'week', bot.interval
    assert_equal @usd.id, bot.quote_asset_id
    assert_nil session[:bot_config], 'wizard session must be cleared after finalise'
    assert_match 'turbo-stream', response.body
    assert_match bot_path(bot), response.body
  end

  # The blank-param and failed-save 422 re-render branches are characterized in
  # wizard_create_failure_rerender_test.rb (for every wizard step).

  test 'create with an expired wizard session turbo-redirects to root' do
    # No seeding: session[:bot_config] is blank.
    post bots_dca_single_assets_pick_spendable_asset_path,
         params: { bots_dca_single_asset: { quote_asset_id: @usd.id } }
    assert_response :success
    assert_match 'turbo-stream', response.body
    assert_match %(action="redirect"), response.body
  end
end

class Bots::DcaDualAssets::PickSpendableAssetsControllerTest < ActionDispatch::IntegrationTest
  setup do
    @user = create(:user, admin: true, setup_completed: true)
    sign_in @user
    @btc = create(:asset, :bitcoin)
    @eth = create(:asset, :ethereum)
    @usd = create(:asset, :usd)
    @binance = create(:binance_exchange)
    create(:ticker, :btc_usd, exchange: @binance, base_asset: @btc, quote_asset: @usd)
    create(:ticker, :eth_usd, exchange: @binance, base_asset: @eth, quote_asset: @usd)
  end

  def seed_wizard_session
    get new_bots_dca_single_assets_pick_buyable_asset_path
    post bots_dca_single_assets_pick_buyable_asset_path,
         params: { bots_dca_single_asset: { base_asset_id: @btc.id } }
    post promote_to_dual_bots_dca_single_assets_pick_exchange_path
    post bots_dca_dual_assets_pick_second_buyable_asset_path,
         params: { bots_dca_dual_asset: { base1_asset_id: @eth.id } }
    post bots_dca_dual_assets_pick_exchange_path,
         params: { bots_dca_dual_asset: { exchange_id: @binance.id } }
  end

  test 'create finalises an unstarted bot with 100/week defaults and allocation0 0.5' do
    seed_wizard_session
    assert_difference -> { @user.bots.count }, 1 do
      post bots_dca_dual_assets_pick_spendable_asset_path,
           params: { bots_dca_dual_asset: { quote_asset_id: @usd.id } }
    end
    assert_response :success

    bot = @user.bots.order(:id).last
    assert_instance_of Bots::DcaDualAsset, bot
    assert_predicate bot, :created?
    assert_equal 100, bot.quote_amount
    assert_equal 'week', bot.interval
    assert_equal 0.5, bot.allocation0
    assert_nil session[:bot_config]
    assert_match bot_path(bot), response.body
  end

  test 'create with an expired wizard session turbo-redirects to root' do
    post bots_dca_dual_assets_pick_spendable_asset_path,
         params: { bots_dca_dual_asset: { quote_asset_id: @usd.id } }
    assert_response :success
    assert_match %(action="redirect"), response.body
  end
end

class Bots::DcaIndexes::PickSpendableAssetsControllerTest < ActionDispatch::IntegrationTest
  setup do
    @user = create(:user, admin: true, setup_completed: true)
    sign_in @user
    MarketDataSettings.stubs(:current_provider).returns(MarketDataSettings::PROVIDER_DELTABADGER)
    @eur = create(:asset, :eur)
    @kraken = create(:kraken_exchange)
    btc = create(:asset, :bitcoin)
    create(:ticker, exchange: @kraken, base_asset: btc, quote_asset: @eur)
  end

  def seed_wizard_session
    post bots_dca_indexes_pick_index_path, params: { index_type: Bots::DcaIndex::INDEX_TYPE_TOP }
    post bots_dca_indexes_pick_exchange_path,
         params: { bots_dca_index: { exchange_id: @kraken.id } }
  end

  test 'create finalises an unstarted bot with 100/week defaults and allocation_flattening 0.0' do
    seed_wizard_session
    assert_difference -> { @user.bots.count }, 1 do
      post bots_dca_indexes_pick_spendable_asset_path,
           params: { bots_dca_index: { quote_asset_id: @eur.id } }
    end
    assert_response :success

    bot = @user.bots.order(:id).last
    assert_instance_of Bots::DcaIndex, bot
    assert_predicate bot, :created?
    assert_equal 100, bot.quote_amount
    assert_equal 'week', bot.interval
    assert_equal 0.0, bot.allocation_flattening
    assert_nil session[:bot_config]
    assert_match bot_path(bot), response.body
  end

  test 'new redirects to coingecko setup when market data is not configured' do
    MarketDataSettings.unstub(:current_provider)
    get new_bots_dca_indexes_pick_spendable_asset_path
    assert_redirected_to new_bots_dca_indexes_setup_coingecko_path
  end
end

class Bots::Signals::PickSpendableAssetsControllerTest < ActionDispatch::IntegrationTest
  setup do
    @user = create(:user, admin: true, setup_completed: true)
    sign_in @user
    @btc = create(:asset, :bitcoin)
    @usd = create(:asset, :usd)
    @binance = create(:binance_exchange)
    create(:ticker, :btc_usd, exchange: @binance, base_asset: @btc, quote_asset: @usd)
  end

  def seed_wizard_session
    get new_bots_signals_pick_buyable_asset_path
    post bots_signals_pick_buyable_asset_path,
         params: { bots_signal: { base_asset_id: @btc.id } }
    post bots_signals_pick_exchange_path,
         params: { bots_signal: { exchange_id: @binance.id } }
  end

  test 'create merges the quote asset into the session and hands off to confirm_settings without creating a bot' do
    seed_wizard_session
    assert_no_difference -> { Bot.count } do
      post bots_signals_pick_spendable_asset_path,
           params: { bots_signal: { quote_asset_id: @usd.id } }
    end
    assert_redirected_to new_bots_signals_confirm_settings_path
    assert_equal @usd.id, session[:bot_config].dig('settings', 'quote_asset_id')
  end

  test 'new redirects to the api-key step when the key is not correct' do
    seed_wizard_session
    ApiKey.any_instance.stubs(:correct?).returns(false)
    get new_bots_signals_pick_spendable_asset_path
    assert_redirected_to new_bots_signals_add_api_key_path
  end
end
