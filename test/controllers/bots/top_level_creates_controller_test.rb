require 'test_helper'

# Characterization tests for the four top-level <type>s_controller#create actions,
# written before extracting a shared base. All four: build from session[:bot_config]
# → save && start(start_fresh: true) → clear session → turbo-stream redirect to the
# bot page; on failure → flash + 422 turbo stream, session kept. Signals additionally
# build bot_signals from the session (and skip set_missed_quote_amount — Bots::Signal
# does not include Accountable).
#
# These are functional (ActionController::TestCase) tests: the wizard session is
# seeded directly because the live wizard finalises bots in pick_spendable_assets
# (clearing the session), so a full session config is not reachable through requests
# for single/dual/index. The create actions remain routed and reachable directly
# (and via the legacy index/signals confirm_settings forms).
module TopLevelCreateBehavior
  extend ActiveSupport::Concern

  included do
    include Devise::Test::ControllerHelpers

    setup do
      @user = create(:user, admin: true, setup_completed: true)
      sign_in @user
    end

    test 'create builds the bot from the session, starts it, and turbo-redirects to the bot page' do
      assert_difference -> { @user.bots.count }, 1 do
        post :create, session: { bot_config: valid_bot_config }
      end
      assert_response :success

      bot = @user.bots.order(:id).last
      assert_instance_of expected_bot_class, bot
      assert_predicate bot, :scheduled?, 'create must start the bot (start_fresh: true)'
      assert_nil session[:bot_config], 'wizard session must be cleared'
      assert_match 'turbo-stream', response.body
      assert_match %(action="redirect"), response.body
    end

    test 'create with an invalid config keeps the session and re-renders the flash with 422' do
      # `save && start` is not transactional: a bot that saves but fails :start
      # validations (signals with no exchange) is left persisted in :created state.
      assert_difference -> { Bot.count }, invalid_create_persisted_bots do
        post :create, session: { bot_config: invalid_bot_config }
      end
      assert_response :unprocessable_entity
      assert_match 'turbo-stream', response.body
      assert_match 'flash', response.body
      assert_not_nil session[:bot_config], 'session must be kept so the user can correct the config'
    end
  end

  private

  # Single/dual/index fail at save (quote_amount presence) → nothing persisted.
  # Signals overrides: its bare config saves fine and only fails on :start.
  def invalid_create_persisted_bots = 0
end

class Bots::DcaSingleAssetsControllerTest < ActionController::TestCase
  include TopLevelCreateBehavior

  private

  def expected_bot_class = Bots::DcaSingleAsset

  def valid_bot_config
    btc = create(:asset, :bitcoin)
    usd = create(:asset, :usd)
    binance = create(:binance_exchange)
    create(:ticker, :btc_usd, exchange: binance, base_asset: btc, quote_asset: usd)
    {
      'label' => 'Test Bot',
      'exchange_id' => binance.id,
      'settings' => {
        'base_asset_id' => btc.id,
        'quote_asset_id' => usd.id,
        'quote_amount' => 100,
        'interval' => 'week'
      }
    }
  end

  def invalid_bot_config
    # No quote_amount → quote_amount presence validation fails.
    { 'label' => 'Test Bot', 'settings' => { 'interval' => 'week' } }
  end
end

class Bots::DcaDualAssetsControllerTest < ActionController::TestCase
  include TopLevelCreateBehavior

  private

  def expected_bot_class = Bots::DcaDualAsset

  def valid_bot_config
    btc = create(:asset, :bitcoin)
    eth = create(:asset, :ethereum)
    usd = create(:asset, :usd)
    binance = create(:binance_exchange)
    create(:ticker, :btc_usd, exchange: binance, base_asset: btc, quote_asset: usd)
    create(:ticker, :eth_usd, exchange: binance, base_asset: eth, quote_asset: usd)
    {
      'label' => 'Test Bot',
      'exchange_id' => binance.id,
      'settings' => {
        'base0_asset_id' => btc.id,
        'base1_asset_id' => eth.id,
        'quote_asset_id' => usd.id,
        'quote_amount' => 100,
        'allocation0' => 0.5,
        'interval' => 'week'
      }
    }
  end

  def invalid_bot_config
    { 'label' => 'Test Bot', 'settings' => { 'interval' => 'week' } }
  end
end

class Bots::DcaIndexesControllerTest < ActionController::TestCase
  include TopLevelCreateBehavior

  setup do
    MarketDataSettings.stubs(:current_provider).returns(MarketDataSettings::PROVIDER_DELTABADGER)
  end

  private

  def expected_bot_class = Bots::DcaIndex

  def valid_bot_config
    eur = create(:asset, :eur)
    kraken = create(:kraken_exchange)
    btc = create(:asset, :bitcoin)
    create(:ticker, exchange: kraken, base_asset: btc, quote_asset: eur)
    {
      'label' => 'Test Bot',
      'exchange_id' => kraken.id,
      'settings' => {
        'quote_asset_id' => eur.id,
        'quote_amount' => 100,
        'interval' => 'week',
        'allocation_flattening' => 0.0,
        'index_type' => Bots::DcaIndex::INDEX_TYPE_TOP
      }
    }
  end

  def invalid_bot_config
    { 'label' => 'Test Bot', 'settings' => { 'index_type' => Bots::DcaIndex::INDEX_TYPE_TOP } }
  end
end

class Bots::SignalsControllerTest < ActionController::TestCase
  include TopLevelCreateBehavior

  test 'create builds bot_signals from the session config' do
    config = valid_bot_config.merge(
      'signals' => [
        { 'direction' => 'buy', 'amount' => 100, 'enabled' => true },
        { 'direction' => 'sell', 'amount' => 25.5, 'amount_type' => 'percentage', 'enabled' => false }
      ]
    )

    post :create, session: { bot_config: config }
    assert_response :success

    bot = @user.bots.order(:id).last
    assert_equal 2, bot.bot_signals.count
    buy, sell = bot.bot_signals.order(:id).to_a
    assert_predicate buy, :buy?
    assert_equal 100, buy.amount
    assert buy.enabled
    assert_equal 'fixed', buy.amount_type, 'amount_type defaults to fixed when absent'
    assert_predicate sell, :sell?
    assert_equal 25.5, sell.amount
    assert_equal 'percentage', sell.amount_type
    assert_not sell.enabled
  end

  test 'create without session signals falls back to a single enabled buy signal' do
    post :create, session: { bot_config: valid_bot_config }
    assert_response :success

    bot = @user.bots.order(:id).last
    assert_equal 1, bot.bot_signals.count
    signal = bot.bot_signals.first
    assert_predicate signal, :buy?
    assert_equal 100, signal.amount
    assert signal.enabled
  end

  private

  def expected_bot_class = Bots::Signal

  def valid_bot_config
    btc = create(:asset, :bitcoin)
    usd = create(:asset, :usd)
    binance = create(:binance_exchange)
    create(:ticker, :btc_usd, exchange: binance, base_asset: btc, quote_asset: usd)
    {
      'label' => 'Test Bot',
      'exchange_id' => binance.id,
      'settings' => {
        'base_asset_id' => btc.id,
        'quote_asset_id' => usd.id
      }
    }
  end

  def invalid_bot_config
    # A bare signal bot saves fine; it fails the :start validations (no ticker),
    # hitting the flash + 422 branch with the bot left persisted (see module note).
    { 'label' => 'Test Bot', 'settings' => {} }
  end

  def invalid_create_persisted_bots = 1
end
