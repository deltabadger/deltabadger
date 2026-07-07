require 'test_helper'

class SettingsStocksTest < ActionDispatch::IntegrationTest
  include Devise::Test::IntegrationHelpers

  setup do
    @admin = create(:user, admin: true, setup_completed: true)
    @exchange = create(:alpaca_exchange)
    sign_in @admin
  end

  test 'stocks widget shows OFF when not configured' do
    get settings_connect_path
    assert_response :success
    assert_select 'turbo-frame#stocks_settings'
  end

  test 'enable stocks seeds the admin trading key when absent' do
    Clients::Alpaca.any_instance.stubs(:get_account).returns(
      Result::Success.new({ 'status' => 'ACTIVE' })
    )
    Exchange::SyncAlpacaAssetsJob.stubs(:perform_later)

    # First connect: the admin has no Alpaca trading key yet, so the validated
    # credentials are inherited as their per-user key (no double entry).
    assert_difference 'ApiKey.count', 1 do
      patch settings_update_stocks_path, params: {
        alpaca_api_key: 'test_key',
        alpaca_api_secret: 'test_secret',
        alpaca_mode: 'paper'
      }
    end

    assert_equal 'test_key', AppConfig.get('alpaca_api_key')
    assert_equal 'test_secret', AppConfig.get('alpaca_api_secret')
    assert_equal 'paper', AppConfig.get('alpaca_mode')

    seeded = @admin.api_keys.find_by(exchange: @exchange, key_type: 'trading')
    assert_equal 'test_key', seeded.key
    assert_equal 'test_secret', seeded.secret
    assert_equal 'paper', seeded.passphrase
    assert seeded.correct?
  end

  test 'enable stocks never touches an existing admin trading key' do
    existing = create(:api_key, user: @admin, exchange: @exchange,
                                raw_key: 'my_own_key', raw_secret: 'my_own_secret', raw_passphrase: 'live')
    Clients::Alpaca.any_instance.stubs(:get_account).returns(
      Result::Success.new({ 'status' => 'ACTIVE' })
    )
    Exchange::SyncAlpacaAssetsJob.stubs(:perform_later)

    assert_no_difference 'ApiKey.count' do
      patch settings_update_stocks_path, params: {
        alpaca_api_key: 'container_key',
        alpaca_api_secret: 'container_secret',
        alpaca_mode: 'paper'
      }
    end

    assert_equal 'container_key', AppConfig.get('alpaca_api_key')
    existing.reload
    assert_equal 'my_own_key', existing.key
    assert_equal 'my_own_secret', existing.secret
    assert_equal 'live', existing.passphrase
  end

  test 'rotating the sync credential updates AppConfig with no side effects' do
    AppConfig.set('alpaca_api_key', 'old_key')
    AppConfig.set('alpaca_api_secret', 'old_secret')
    AppConfig.set('alpaca_mode', 'paper')
    # Normal post-first-connect state: the admin's key was seeded back then.
    admin_key = create(:api_key, user: @admin, exchange: @exchange,
                                 raw_key: 'old_key', raw_secret: 'old_secret', raw_passphrase: 'paper')
    usd = create(:asset, :usd)
    aapl = create(:asset, external_id: 'alpaca_uuid-aapl', symbol: 'AAPL', name: 'Apple Inc', category: 'Stock')
    other_user = create(:user, setup_completed: true)
    other_bot = create(:dca_single_asset, :waiting, user: other_user, exchange: @exchange,
                                                    base_asset: aapl, quote_asset: usd)
    other_key = other_user.api_keys.find_by(exchange: @exchange)
    other_key.update!(status: :correct)
    old_key_value = other_key.key

    Clients::Alpaca.any_instance.stubs(:get_account).returns(
      Result::Success.new({ 'status' => 'ACTIVE' })
    )
    Exchange::SyncAlpacaAssetsJob.expects(:perform_later)

    assert_no_difference 'ApiKey.count' do
      patch settings_update_stocks_path, params: {
        alpaca_api_key: 'new_key',
        alpaca_api_secret: 'new_secret',
        alpaca_mode: 'live'
      }
    end

    assert_equal 'new_key', AppConfig.get('alpaca_api_key')
    assert_equal 'new_secret', AppConfig.get('alpaca_api_secret')
    assert_equal 'live', AppConfig.get('alpaca_mode')

    # No user's bots, keys, or tickers move on an admin rotation — including
    # the admin's own previously seeded key.
    assert other_bot.reload.waiting?
    assert other_key.reload.correct?
    assert_equal old_key_value, other_key.key
    assert_equal 'old_key', admin_key.reload.key
    assert Ticker.find_by(exchange: @exchange, base_asset: aapl).available?
    assert_equal 0, SolidQueue::Job.where(class_name: 'Bot::StopJob').count
  end

  test 'stocks widget offers the rotation form alongside disconnect when configured' do
    AppConfig.set('alpaca_api_key', 'test_key')
    AppConfig.set('alpaca_api_secret', 'test_secret')
    AppConfig.set('alpaca_mode', 'paper')

    get settings_connect_path

    assert_response :success
    assert_select 'turbo-frame#stocks_settings input[name=alpaca_api_key]'
    assert_select "turbo-frame#stocks_settings a[href='#{settings_disconnect_stocks_path}']"
    assert_select 'turbo-frame#stocks_settings input[name=alpaca_mode][value=paper][checked]'
  end

  test 'stocks widget shows ON with a sync note when a catalog exists without a credential' do
    # Post-disconnect state: catalog synced earlier, credential removed.
    usd = create(:asset, :usd)
    aapl = create(:asset, external_id: 'alpaca_uuid-aapl', symbol: 'AAPL', name: 'Apple Inc', category: 'Stock')
    create(:ticker, exchange: @exchange, base_asset: aapl, quote_asset: usd, available: true)

    get settings_connect_path

    assert_response :success
    assert_select 'turbo-frame#stocks_settings .text-success', text: 'ON'
    assert_includes response.body, I18n.t('settings.stocks.not_refreshing')
    assert_select 'turbo-frame#stocks_settings input[name=alpaca_api_key]'
    assert_select "turbo-frame#stocks_settings a[href='#{settings_disconnect_stocks_path}']", count: 0
  end

  test 'enable stocks with invalid API key shows error' do
    Clients::Alpaca.any_instance.stubs(:get_account).returns(
      Result::Failure.new('Unauthorized')
    )

    patch settings_update_stocks_path, params: {
      alpaca_api_key: 'bad_key',
      alpaca_api_secret: 'bad_secret',
      alpaca_mode: 'paper'
    }

    assert_response :unprocessable_entity
    assert_nil AppConfig.get('alpaca_api_key')
  end

  test 'disconnect clears the sync credential and has zero side effects' do
    AppConfig.set('alpaca_api_key', 'test_key')
    AppConfig.set('alpaca_api_secret', 'test_secret')
    AppConfig.set('alpaca_mode', 'paper')

    usd = create(:asset, :usd)
    aapl = create(:asset, external_id: 'alpaca_uuid-aapl', symbol: 'AAPL', name: 'Apple Inc', category: 'Stock')
    msft = create(:asset, external_id: 'alpaca_uuid-msft', symbol: 'MSFT', name: 'Microsoft', category: 'Stock')
    other_user = create(:user, setup_completed: true)

    admin_bot = create(:dca_single_asset, :waiting, user: @admin, exchange: @exchange,
                                                    base_asset: aapl, quote_asset: usd)
    other_bot = create(:dca_single_asset, :waiting, user: other_user, exchange: @exchange,
                                                    base_asset: aapl, quote_asset: usd)
    dual_bot = create(:dca_dual_asset, :waiting, user: other_user, exchange: @exchange,
                                                 base0_asset: aapl, base1_asset: msft, quote_asset: usd)

    other_key = other_user.api_keys.find_by(exchange: @exchange)
    other_key.update!(status: :correct)

    assert_no_difference 'ApiKey.count' do
      delete settings_disconnect_stocks_path
    end

    # The credential is a catalog-sync bootstrap — dropping it must not linger
    # encrypted in the DB, and must not touch anything else.
    assert_nil AppConfig.get('alpaca_api_key')
    assert_nil AppConfig.get('alpaca_api_secret')
    assert_nil AppConfig.get('alpaca_mode')

    # Catalog untouched: pickers keep listing stocks.
    assert Ticker.find_by(exchange: @exchange, base_asset: aapl).available?

    # Bots keep running on their owners' per-user keys; keys intact.
    assert admin_bot.reload.waiting?
    assert other_bot.reload.waiting?
    assert dual_bot.reload.waiting?
    assert_equal 0, SolidQueue::Job.where(class_name: 'Bot::StopJob').count
    assert other_key.reload.correct?

    assert_includes response.body, I18n.t('settings.stocks.disconnected')
  end

  test 'update_stocks with blank credentials is a validation error, not a disable' do
    AppConfig.set('alpaca_api_key', 'test_key')
    AppConfig.set('alpaca_api_secret', 'test_secret')
    AppConfig.set('alpaca_mode', 'paper')
    usd = create(:asset, :usd)
    aapl = create(:asset, external_id: 'alpaca_uuid-aapl', symbol: 'AAPL', name: 'Apple Inc', category: 'Stock')
    bot = create(:dca_single_asset, :waiting, user: @admin, exchange: @exchange,
                                              base_asset: aapl, quote_asset: usd)

    assert_no_difference 'ApiKey.count' do
      patch settings_update_stocks_path, params: { alpaca_api_key: '', alpaca_api_secret: '' }
    end

    assert_response :unprocessable_entity
    assert_includes response.body, I18n.t('settings.stocks.missing_credentials')
    # Nothing changed: credential, catalog, and bots are exactly as before.
    assert_equal 'test_key', AppConfig.get('alpaca_api_key')
    assert_equal 'test_secret', AppConfig.get('alpaca_api_secret')
    assert Ticker.find_by(exchange: @exchange, base_asset: aapl).available?
    assert bot.reload.waiting?
    assert_equal 0, SolidQueue::Job.where(class_name: 'Bot::StopJob').count
  end

  # The deltabadger radio only renders when the env feed exists; a crafted request (or a hosted
  # DB later run self-hosted, via the seeds default) must not select a provider the container
  # cannot reach — it wedges the stocks endpoints behind the 422 hosted guard.
  test 'update_market_data rejects the deltabadger provider without a data feed env' do
    patch settings_update_market_data_path, params: { market_data_provider: 'deltabadger' }

    assert_response :unprocessable_entity
    assert_nil AppConfig.market_data_provider
  end

  test 'stocks endpoints are rejected on hosted (catalog comes from the platform)' do
    MarketDataSettings.stubs(:current_provider).returns(MarketDataSettings::PROVIDER_DELTABADGER)

    patch settings_update_stocks_path, params: { alpaca_api_key: 'k', alpaca_api_secret: 's' }
    assert_response :unprocessable_entity
    assert_nil AppConfig.get('alpaca_api_key')

    delete settings_disconnect_stocks_path
    assert_response :unprocessable_entity
  end

  test 'stocks widget on hosted shows platform status and no key form' do
    MarketDataSettings.stubs(:current_provider).returns(MarketDataSettings::PROVIDER_DELTABADGER)
    AppConfig.stubs(:market_data_env_provider_name).returns('Deltabadger Cloud')

    get settings_connect_path

    assert_response :success
    assert_match 'Deltabadger Cloud', response.body
    assert_select 'input[name=alpaca_api_key]', count: 0
    assert_select "a[href='#{settings_disconnect_stocks_path}']", count: 0
  end

  test 'non-admin sees the IBKR widget but not the stocks widget' do
    sign_in create(:user, setup_completed: true)
    create(:ibkr_exchange)

    get settings_connect_path

    assert_response :success
    assert_select 'turbo-frame#stocks_settings', count: 0
    assert_match I18n.t('settings.ibkr.widget_title'), response.body
  end

  test 'stocks endpoints are admin-only' do
    sign_in create(:user, setup_completed: true)

    patch settings_update_stocks_path, params: { alpaca_api_key: 'k', alpaca_api_secret: 's' }
    assert_response :forbidden

    delete settings_disconnect_stocks_path
    assert_response :forbidden
  end
end
