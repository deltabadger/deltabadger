require 'test_helper'

# Characterization tests for the add-API-keys wizard step controllers, written before
# extracting a shared step base class. Complements add_api_keys_views_test.rb (which
# pins the form rendering): here we pin the routing/branching — correct key skips
# ahead, incorrect/unverifiable keys re-render with the right flash, a missing
# exchange sends each type back to its own previous step, and the Alpaca sync hook
# fires for single/dual only.
module AddApiKeyStepBranchTests
  extend ActiveSupport::Concern

  # The api-key form posts as a turbo stream; the failure branches only have
  # create.turbo_stream.erb templates (same as add_api_keys_views_test.rb).
  TURBO_STREAM_ACCEPT = 'text/vnd.turbo-stream.html, text/html'.freeze

  included do
    setup do
      @user = create(:user, admin: true, setup_completed: true)
      sign_in @user
    end

    test 'new with an already-correct key skips ahead to the spendable-asset step' do
      seed_wizard_session
      # In tests (dry run) bot.api_key is always status: :correct.
      get add_api_key_path
      assert_redirected_to after_api_key_path
    end

    test 'create with valid credentials turbo-redirects to the spendable-asset step' do
      seed_wizard_session
      ApiKey.any_instance.stubs(:validate_credentials!)
      ApiKey.any_instance.stubs(:correct?).returns(true)

      post add_api_keys_path, params: { api_key: { key: 'k', secret: 's' } }
      assert_response :success
      assert_match 'turbo-stream', response.body
      assert_match after_api_key_path, response.body
    end

    test 'create with incorrect credentials re-renders with the permissions error' do
      seed_wizard_session
      ApiKey.any_instance.stubs(:validate_credentials!)
      ApiKey.any_instance.stubs(:correct?).returns(false)
      ApiKey.any_instance.stubs(:incorrect?).returns(true)

      post add_api_keys_path, params: { api_key: { key: 'k', secret: 's' } },
                              headers: { 'Accept' => TURBO_STREAM_ACCEPT }
      assert_response :unprocessable_entity
      assert_match I18n.t('errors.incorrect_api_key_permissions'), response.body
    end

    test 'create when validation cannot complete re-renders with the validation-failed error' do
      seed_wizard_session
      ApiKey.any_instance.stubs(:validate_credentials!)
      ApiKey.any_instance.stubs(:correct?).returns(false)
      ApiKey.any_instance.stubs(:incorrect?).returns(false)

      post add_api_keys_path, params: { api_key: { key: 'k', secret: 's' } },
                              headers: { 'Accept' => TURBO_STREAM_ACCEPT }
      assert_response :unprocessable_entity
      assert_match I18n.t('errors.api_key_permission_validation_failed'), response.body
    end

    test 'new without an exchange in the session redirects to the previous step' do
      seed_session_without_exchange
      get add_api_key_path
      assert_redirected_to missing_exchange_redirect_path
    end
  end
end

class Bots::DcaSingleAssets::AddApiKeysControllerTest < ActionDispatch::IntegrationTest
  include AddApiKeyStepBranchTests

  private

  def add_api_key_path = new_bots_dca_single_assets_add_api_key_path
  def add_api_keys_path = bots_dca_single_assets_add_api_key_path
  def after_api_key_path = new_bots_dca_single_assets_pick_spendable_asset_path
  def missing_exchange_redirect_path = new_bots_dca_single_assets_pick_exchange_path

  def seed_assets
    @btc = create(:asset, :bitcoin)
    @usd = create(:asset, :usd)
    @binance = create(:binance_exchange)
    create(:ticker, :btc_usd, exchange: @binance, base_asset: @btc, quote_asset: @usd)
  end

  def seed_session_without_exchange
    seed_assets
    get new_bots_dca_single_assets_pick_buyable_asset_path
    post bots_dca_single_assets_pick_buyable_asset_path,
         params: { bots_dca_single_asset: { base_asset_id: @btc.id } }
  end

  def seed_wizard_session
    seed_session_without_exchange
    post bots_dca_single_assets_pick_exchange_path,
         params: { bots_dca_single_asset: { exchange_id: @binance.id } }
  end
end

# Container-global activation is admin/Settings-only: a per-user Alpaca connect
# in bot creation must touch ONLY the connecting user's ApiKey.
class Bots::DcaSingleAssets::AddApiKeysAlpacaIsolationTest < ActionDispatch::IntegrationTest
  setup do
    # Simulate a self-hosted container the admin already activated: the
    # container sync credential belongs to the admin.
    AppConfig.set('alpaca_api_key', 'admin-key')
    AppConfig.set('alpaca_api_secret', 'admin-secret')
    AppConfig.set('alpaca_mode', 'paper')

    @admin = create(:user, admin: true, setup_completed: true)
    @user = create(:user, setup_completed: true)
    sign_in @user

    usd = create(:asset, :usd)
    aapl = create(:asset, symbol: 'AAPL', name: 'Apple Inc', category: 'Stock', external_id: 'aapl')
    @alpaca = create(:alpaca_exchange)
    create(:ticker, exchange: @alpaca, base_asset: aapl, quote_asset: usd, base: 'AAPL', quote: 'USD')

    # Single stock venue: the asset POST auto-selects Alpaca into the session.
    get new_bots_dca_single_assets_pick_buyable_asset_path
    post bots_dca_single_assets_pick_buyable_asset_path,
         params: { bots_dca_single_asset: { base_asset_id: aapl.id } }
  end

  test 'a non-admin Alpaca connect creates only their own ApiKey and never touches AppConfig' do
    # Let validate_credentials! assign the posted credentials for real; only the
    # remote validity check is stubbed out.
    ApiKey.any_instance.stubs(:get_validity).returns(Result::Success.new(true))
    Exchange::SyncAlpacaAssetsJob.expects(:perform_later).never

    post bots_dca_single_assets_add_api_key_path,
         params: { api_key: { key: 'user-key', secret: 'user-secret', passphrase: 'live' } }

    assert_response :success
    # Container sync credential (admin's) is untouched — no last-writer-wins.
    assert_equal 'admin-key', AppConfig.get('alpaca_api_key')
    assert_equal 'admin-secret', AppConfig.get('alpaca_api_secret')
    assert_equal 'paper', AppConfig.get('alpaca_mode')
    # The connecting user got their own key, in their own mode.
    user_key = @user.api_keys.find_by(exchange: @alpaca, key_type: :trading)
    assert_equal 'user-key', user_key.key
    assert_equal 'live', user_key.passphrase
    # And nothing was created for the admin.
    assert_nil @admin.api_keys.find_by(exchange: @alpaca)
  end
end

# Same isolation guarantee for the non-wizard "re-add key on an existing bot" flow.
class Bots::AddApiKeysAlpacaIsolationTest < ActionDispatch::IntegrationTest
  test 'posting Alpaca credentials for an existing bot never writes AppConfig' do
    create(:user, admin: true, setup_completed: true) # container is set up
    user = create(:user, setup_completed: true)
    sign_in user
    alpaca = create(:alpaca_exchange)
    aapl = create(:asset, symbol: 'AAPL', name: 'Apple Inc', category: 'Stock', external_id: 'aapl')
    usd = create(:asset, :usd)
    bot = create(:dca_single_asset, user: user, exchange: alpaca,
                                    base_asset: aapl, quote_asset: usd, with_api_key: false)
    ApiKey.any_instance.stubs(:get_validity).returns(Result::Success.new(true))
    Exchange::SyncAlpacaAssetsJob.expects(:perform_later).never

    post bot_add_api_key_path(bot_id: bot.id), params: { api_key: { key: 'k', secret: 's' } }

    assert_response :success
    assert_nil AppConfig.get('alpaca_api_key')
    assert_nil AppConfig.get('alpaca_api_secret')
    assert_nil AppConfig.get('alpaca_mode')

    # Paper-by-default: a passphrase-less connect must never persist a live-mode key
    # (Exchanges::Alpaca#paper_mode? treats anything but 'live' as paper).
    user_key = user.api_keys.find_by(exchange: alpaca, key_type: :trading)
    refute_equal 'live', user_key.passphrase
  end
end

class Bots::DcaDualAssets::AddApiKeysControllerTest < ActionDispatch::IntegrationTest
  include AddApiKeyStepBranchTests

  private

  def add_api_key_path = new_bots_dca_dual_assets_add_api_key_path
  def add_api_keys_path = bots_dca_dual_assets_add_api_key_path
  def after_api_key_path = new_bots_dca_dual_assets_pick_spendable_asset_path
  def missing_exchange_redirect_path = new_bots_dca_dual_assets_pick_exchange_path

  def seed_assets
    @btc = create(:asset, :bitcoin)
    @eth = create(:asset, :ethereum)
    @usd = create(:asset, :usd)
    @binance = create(:binance_exchange)
    create(:ticker, :btc_usd, exchange: @binance, base_asset: @btc, quote_asset: @usd)
    create(:ticker, :eth_usd, exchange: @binance, base_asset: @eth, quote_asset: @usd)
  end

  def seed_session_without_exchange
    seed_assets
    get new_bots_dca_single_assets_pick_buyable_asset_path
    post bots_dca_single_assets_pick_buyable_asset_path,
         params: { bots_dca_single_asset: { base_asset_id: @btc.id } }
    post promote_to_dual_bots_dca_single_assets_pick_exchange_path
    post bots_dca_dual_assets_pick_second_buyable_asset_path,
         params: { bots_dca_dual_asset: { base1_asset_id: @eth.id } }
  end

  def seed_wizard_session
    seed_session_without_exchange
    post bots_dca_dual_assets_pick_exchange_path,
         params: { bots_dca_dual_asset: { exchange_id: @binance.id } }
  end
end

class Bots::DcaIndexes::AddApiKeysControllerTest < ActionDispatch::IntegrationTest
  include AddApiKeyStepBranchTests

  setup do
    MarketDataSettings.stubs(:current_provider).returns(MarketDataSettings::PROVIDER_DELTABADGER)
  end

  test 'new redirects to coingecko setup when market data is not configured' do
    MarketDataSettings.unstub(:current_provider)
    get new_bots_dca_indexes_add_api_key_path
    assert_redirected_to new_bots_dca_indexes_setup_coingecko_path
  end

  private

  def add_api_key_path = new_bots_dca_indexes_add_api_key_path
  def add_api_keys_path = bots_dca_indexes_add_api_key_path
  def after_api_key_path = new_bots_dca_indexes_pick_spendable_asset_path
  def missing_exchange_redirect_path = new_bots_dca_indexes_pick_exchange_path

  def seed_session_without_exchange
    post bots_dca_indexes_pick_index_path, params: { index_type: Bots::DcaIndex::INDEX_TYPE_TOP }
  end

  def seed_wizard_session
    seed_session_without_exchange
    @kraken = create(:kraken_exchange)
    post bots_dca_indexes_pick_exchange_path,
         params: { bots_dca_index: { exchange_id: @kraken.id } }
  end
end

class Bots::Signals::AddApiKeysControllerTest < ActionDispatch::IntegrationTest
  include AddApiKeyStepBranchTests

  private

  def add_api_key_path = new_bots_signals_add_api_key_path
  def add_api_keys_path = bots_signals_add_api_key_path
  def after_api_key_path = new_bots_signals_pick_spendable_asset_path
  def missing_exchange_redirect_path = new_bots_signals_pick_exchange_path

  def seed_session_without_exchange
    @btc = create(:asset, :bitcoin)
    @usd = create(:asset, :usd)
    @binance = create(:binance_exchange)
    create(:ticker, :btc_usd, exchange: @binance, base_asset: @btc, quote_asset: @usd)
    get new_bots_signals_pick_buyable_asset_path
    post bots_signals_pick_buyable_asset_path,
         params: { bots_signal: { base_asset_id: @btc.id } }
  end

  def seed_wizard_session
    seed_session_without_exchange
    post bots_signals_pick_exchange_path,
         params: { bots_signal: { exchange_id: @binance.id } }
  end
end
