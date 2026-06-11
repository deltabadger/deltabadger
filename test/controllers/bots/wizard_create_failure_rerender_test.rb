require 'test_helper'

# The wizard step controllers' `create` failure branches (`render :new, status:
# :unprocessable_entity`) must re-render the step, not 500. The `new` templates
# need view state (@bot, @assets/@exchanges, pagination) that historically was
# only set up in `new`, so these branches crashed with "undefined method
# 'base_asset' for nil" in bots/wizard/_sentence. Unreachable from the real UI
# (tiles always post an id) except for the save-failure branch of
# finalise_and_redirect, but reachable via hand-crafted POSTs.
#
# Wizard sessions are seeded through real requests, same pattern as
# Bots::DcaSingleAssets::PickExchangesControllerTest.

class DcaSingleAssetWizardCreateFailureTest < ActionDispatch::IntegrationTest
  setup do
    @user = create(:user, admin: true, setup_completed: true)
    sign_in @user
    @btc = create(:asset, :bitcoin)
    @usd = create(:asset, :usd)
    @binance = create(:binance_exchange)
    create(:ticker, :btc_usd, exchange: @binance, base_asset: @btc, quote_asset: @usd)
  end

  test 'pick_buyable_assets create with a blank base re-renders the asset list with 422' do
    get new_bots_dca_single_assets_pick_buyable_asset_path
    post bots_dca_single_assets_pick_buyable_asset_path,
         params: { bots_dca_single_asset: { base_asset_id: '' } }

    assert_response :unprocessable_entity
    assert_match 'BTC', response.body
  end

  test 'pick_exchanges create with a blank exchange re-renders the exchange list with 422' do
    seed_base_pick
    post bots_dca_single_assets_pick_exchange_path,
         params: { bots_dca_single_asset: { exchange_id: '' } }

    assert_response :unprocessable_entity
    # Exchanges render as submit buttons valued by exchange.id (logos are SVGs, no name text).
    assert_match(/value="#{@binance.id}"/, response.body)
  end

  test 'pick_spendable_assets create with a blank quote re-renders the asset list with 422' do
    seed_through_exchange
    post bots_dca_single_assets_pick_spendable_asset_path,
         params: { bots_dca_single_asset: { quote_asset_id: '' } }

    assert_response :unprocessable_entity
    assert_match 'USD', response.body
  end

  test 'pick_spendable_assets create re-renders the asset list with 422 when the bot fails to save' do
    seed_through_exchange
    Bots::DcaSingleAsset.any_instance.stubs(:save).returns(false)

    assert_no_difference -> { Bot.count } do
      post bots_dca_single_assets_pick_spendable_asset_path,
           params: { bots_dca_single_asset: { quote_asset_id: @usd.id } }
    end

    assert_response :unprocessable_entity
    assert_match 'USD', response.body
  end

  private

  def seed_base_pick
    get new_bots_dca_single_assets_pick_buyable_asset_path
    post bots_dca_single_assets_pick_buyable_asset_path,
         params: { bots_dca_single_asset: { base_asset_id: @btc.id } }
  end

  def seed_through_exchange
    seed_base_pick
    post bots_dca_single_assets_pick_exchange_path,
         params: { bots_dca_single_asset: { exchange_id: @binance.id } }
  end
end

class DcaDualAssetWizardCreateFailureTest < ActionDispatch::IntegrationTest
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

  test 'pick_second_buyable_assets create with a blank second base re-renders the asset list with 422' do
    seed_promoted_to_dual
    post bots_dca_dual_assets_pick_second_buyable_asset_path,
         params: { bots_dca_dual_asset: { base1_asset_id: '' } }

    assert_response :unprocessable_entity
    assert_match 'ETH', response.body
  end

  test 'pick_exchanges create with a blank exchange re-renders the exchange list with 422' do
    seed_through_second_asset
    post bots_dca_dual_assets_pick_exchange_path,
         params: { bots_dca_dual_asset: { exchange_id: '' } }

    assert_response :unprocessable_entity
    assert_match(/value="#{@binance.id}"/, response.body)
  end

  test 'pick_spendable_assets create with a blank quote re-renders the asset list with 422' do
    seed_through_exchange
    post bots_dca_dual_assets_pick_spendable_asset_path,
         params: { bots_dca_dual_asset: { quote_asset_id: '' } }

    assert_response :unprocessable_entity
    assert_match 'USD', response.body
  end

  test 'pick_spendable_assets create re-renders the asset list with 422 when the bot fails to save' do
    seed_through_exchange
    Bots::DcaDualAsset.any_instance.stubs(:save).returns(false)

    assert_no_difference -> { Bot.count } do
      post bots_dca_dual_assets_pick_spendable_asset_path,
           params: { bots_dca_dual_asset: { quote_asset_id: @usd.id } }
    end

    assert_response :unprocessable_entity
    assert_match 'USD', response.body
  end

  private

  def seed_promoted_to_dual
    get new_bots_dca_single_assets_pick_buyable_asset_path
    post bots_dca_single_assets_pick_buyable_asset_path,
         params: { bots_dca_single_asset: { base_asset_id: @btc.id } }
    post promote_to_dual_bots_dca_single_assets_pick_exchange_path
  end

  def seed_through_second_asset
    seed_promoted_to_dual
    post bots_dca_dual_assets_pick_second_buyable_asset_path,
         params: { bots_dca_dual_asset: { base1_asset_id: @eth.id } }
  end

  def seed_through_exchange
    seed_through_second_asset
    post bots_dca_dual_assets_pick_exchange_path,
         params: { bots_dca_dual_asset: { exchange_id: @binance.id } }
  end
end

class DcaIndexWizardCreateFailureTest < ActionDispatch::IntegrationTest
  setup do
    @user = create(:user, admin: true, setup_completed: true)
    sign_in @user
    MarketDataSettings.stubs(:current_provider).returns(MarketDataSettings::PROVIDER_DELTABADGER)
    @eur = create(:asset, :eur)
    @kraken = create(:kraken_exchange)
    # EUR only qualifies as an index quote currency with >= MINIMUM_SUPPORTED_COINS pairs.
    [create(:asset, :bitcoin), create(:asset, :ethereum), create(:asset, symbol: 'SOL', name: 'Solana')]
      .each { |base| create(:ticker, exchange: @kraken, base_asset: base, quote_asset: @eur) }
  end

  test 'pick_exchanges create with a blank exchange re-renders the exchange list with 422' do
    seed_index_pick
    post bots_dca_indexes_pick_exchange_path,
         params: { bots_dca_index: { exchange_id: '' } }

    assert_response :unprocessable_entity
    assert_match(/value="#{@kraken.id}"/, response.body)
  end

  test 'pick_spendable_assets create with a blank quote re-renders the asset list with 422' do
    seed_through_exchange
    post bots_dca_indexes_pick_spendable_asset_path,
         params: { bots_dca_index: { quote_asset_id: '' } }

    assert_response :unprocessable_entity
    assert_match 'EUR', response.body
  end

  test 'pick_spendable_assets create re-renders the asset list with 422 when the bot fails to save' do
    seed_through_exchange
    Bots::DcaIndex.any_instance.stubs(:save).returns(false)

    assert_no_difference -> { Bot.count } do
      post bots_dca_indexes_pick_spendable_asset_path,
           params: { bots_dca_index: { quote_asset_id: @eur.id } }
    end

    assert_response :unprocessable_entity
    assert_match 'EUR', response.body
  end

  private

  def seed_index_pick
    post bots_dca_indexes_pick_index_path, params: { index_type: Bots::DcaIndex::INDEX_TYPE_TOP }
  end

  def seed_through_exchange
    seed_index_pick
    post bots_dca_indexes_pick_exchange_path,
         params: { bots_dca_index: { exchange_id: @kraken.id } }
  end
end

class SignalsWizardCreateFailureTest < ActionDispatch::IntegrationTest
  setup do
    @user = create(:user, admin: true, setup_completed: true)
    sign_in @user
    @btc = create(:asset, :bitcoin)
    @usd = create(:asset, :usd)
    @binance = create(:binance_exchange)
    create(:ticker, :btc_usd, exchange: @binance, base_asset: @btc, quote_asset: @usd)
  end

  test 'pick_buyable_assets create with a blank base re-renders the asset list with 422' do
    get new_bots_signals_pick_buyable_asset_path
    post bots_signals_pick_buyable_asset_path,
         params: { bots_signal: { base_asset_id: '' } }

    assert_response :unprocessable_entity
    assert_match 'BTC', response.body
  end

  test 'pick_exchanges create with a blank exchange re-renders the exchange list with 422' do
    seed_base_pick
    post bots_signals_pick_exchange_path,
         params: { bots_signal: { exchange_id: '' } }

    assert_response :unprocessable_entity
    assert_match(/value="#{@binance.id}"/, response.body)
  end

  test 'pick_spendable_assets create with a blank quote re-renders the asset list with 422' do
    seed_through_exchange
    post bots_signals_pick_spendable_asset_path,
         params: { bots_signal: { quote_asset_id: '' } }

    assert_response :unprocessable_entity
    assert_match 'USD', response.body
  end

  private

  def seed_base_pick
    get new_bots_signals_pick_buyable_asset_path
    post bots_signals_pick_buyable_asset_path,
         params: { bots_signal: { base_asset_id: @btc.id } }
  end

  def seed_through_exchange
    seed_base_pick
    post bots_signals_pick_exchange_path,
         params: { bots_signal: { exchange_id: @binance.id } }
  end
end
