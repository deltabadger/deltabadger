require 'test_helper'

class Settings::PlatformConnectionsControllerTest < ActionDispatch::IntegrationTest
  setup do
    @admin = create(:user, admin: true, setup_completed: true)
    sign_in @admin
  end

  test 'independent settings offers Deltabadger alongside None and CoinGecko' do
    get settings_connect_path

    assert_response :success
    assert_select 'turbo-frame#market_data_settings' do
      assert_select 'input[type=radio][name=market_data_provider][value=""]'
      assert_select 'input[type=radio][name=market_data_provider][value=coingecko]'
      assert_select 'input[type=radio][name=market_data_provider][value=deltabadger]'
      assert_select "form[action='#{settings_platform_connection_path}'] input[name=claim_code]"
    end
  end

  test 'hosted settings keeps the locked provider and has no claim affordance' do
    MarketDataSettings.stubs(:deltabadger_available?).returns(true)
    AppConfig.stubs(:market_data_env_provider_name).returns('Deltabadger Cloud')

    get settings_connect_path

    assert_response :success
    assert_select 'input[type=radio][name=market_data_provider][value=deltabadger][disabled]', count: 1
    assert_select "form[action='#{settings_platform_connection_path}']", count: 0
    assert_select 'input[type=radio][name=market_data_provider][value=coingecko]', count: 0
  end

  test 'admin can connect with a claim code' do
    Platform::RedeemClaim.expects(:call).with(code: 'dbc_settings').returns(
      Result::Success.new(email: 'owner@example.com', name: 'Owner')
    )

    post settings_platform_connection_path, params: { claim_code: 'dbc_settings' }, as: :turbo_stream

    assert_response :success
    assert_select 'turbo-stream[action=replace][target=market_data_settings]'
    assert_select 'turbo-stream[action=prepend][target=flash]'
  end

  test 'switching away from claimed market data keeps the platform status and disconnect control' do
    configure_deltabadger_market_data
    AppConfig.set('platform_connected_at', Time.current.iso8601)
    AppConfig.set('proxy_binance', 'https://proxy.example.com')

    patch settings_update_market_data_path, params: { market_data_provider: '' }, as: :turbo_stream

    assert_response :success
    assert_nil AppConfig.market_data_provider
    assert_equal 'https://proxy.example.com', AppConfig.get('proxy_binance')
    assert_select "a[href='#{settings_platform_connection_path}']", text: I18n.t('settings.platform.disconnect')
    assert_includes response.body, I18n.t('settings.platform.connected_with_proxies')
  end

  test 'switching from claimed market data to CoinGecko preserves and submits the saved key' do
    configure_deltabadger_market_data
    AppConfig.set('platform_connected_at', Time.current.iso8601)
    AppConfig.coingecko_api_key = 'saved-key'

    get settings_connect_path

    assert_response :success
    assert_select 'input[type=hidden][name=coingecko_api_key][value=saved-key]'

    patch settings_update_market_data_path,
          params: { market_data_provider: MarketDataSettings::PROVIDER_COINGECKO,
                    coingecko_api_key: 'saved-key' }, as: :turbo_stream

    assert_response :success
    assert_equal MarketDataSettings::PROVIDER_COINGECKO, AppConfig.market_data_provider
    assert_equal 'saved-key', AppConfig.coingecko_api_key
    assert_select "a[href='#{settings_platform_connection_path}']", text: I18n.t('settings.platform.disconnect')
  end

  test 'a connected install can reselect Deltabadger with its stored claim credentials' do
    configure_deltabadger_market_data
    AppConfig.set('platform_connected_at', Time.current.iso8601)
    AppConfig.market_data_provider = MarketDataSettings::PROVIDER_COINGECKO
    original_url = AppConfig.market_data_url
    original_token = AppConfig.market_data_token

    get settings_connect_path

    assert_response :success
    assert_select 'input[type=radio][name=market_data_provider][value=deltabadger]' \
                  '[data-action="form--market-data#selectConnectedDeltabadger"]'

    patch settings_update_market_data_path,
          params: { market_data_provider: MarketDataSettings::PROVIDER_DELTABADGER }, as: :turbo_stream

    assert_response :success
    assert_equal MarketDataSettings::PROVIDER_DELTABADGER, AppConfig.market_data_provider
    assert_equal original_url, AppConfig.market_data_url
    assert_equal original_token, AppConfig.market_data_token
  end

  test 'platform status says proxies are unavailable when the claim wrote none' do
    AppConfig.set('platform_connected_at', Time.current.iso8601)

    get settings_connect_path

    assert_response :success
    assert_includes response.body, I18n.t('settings.platform.connected_without_proxies')
    assert_not_includes response.body, I18n.t('settings.platform.connected_with_proxies')
  end

  test 'claim failure stays in the market data widget' do
    Platform::RedeemClaim.expects(:call).with(code: 'expired').returns(
      Result::Failure.new('That claim code is invalid or has expired.')
    )

    post settings_platform_connection_path, params: { claim_code: 'expired' }, as: :turbo_stream

    assert_response :unprocessable_entity
    assert_select 'turbo-stream[action=replace][target=market_data_settings]'
    assert_includes response.body, 'That claim code is invalid or has expired.'
  end

  test 'platform connection writes are admin-only' do
    member = create(:user, setup_completed: true)
    sign_in member
    Platform::RedeemClaim.expects(:call).never

    post settings_platform_connection_path, params: { claim_code: 'dbc_attacker' }, as: :turbo_stream

    assert_response :forbidden
  end

  test 'disconnect deletes market data platform and every exact proxy prefix row' do
    AppConfig.market_data_provider = MarketDataSettings::PROVIDER_DELTABADGER
    AppConfig.market_data_url = 'https://claimed-data.example.com'
    AppConfig.market_data_token = 'dbi_claimed'
    AppConfig.set('platform_connected_at', Time.current.iso8601)
    AppConfig.set('proxy_binance', 'http://claimed-binance.example.com')
    AppConfig.set('proxy_custom_exchange', 'http://claimed-custom.example.com')
    AppConfig.set('proxywithoutunderscore', 'keep-me')
    AppConfig.setup_sync_status = AppConfig::SYNC_STATUS_PENDING

    with_env('MARKET_DATA_URL', 'https://env-data.example.com') do
      with_env('MARKET_DATA_TOKEN', 'env-token') do
        with_env('PROXY_BINANCE', 'http://env-binance.example.com') do
          delete settings_platform_connection_path, as: :turbo_stream

          assert_response :success
          assert_equal 'https://env-data.example.com', AppConfig.market_data_url
          assert_equal 'env-token', AppConfig.market_data_token
          assert_equal 'http://env-binance.example.com', ExchangeProxy.for('BINANCE')
        end
      end
    end

    assert_nil AppConfig.find_by(key: AppConfig::MARKET_DATA_PROVIDER)
    assert_nil AppConfig.find_by(key: AppConfig::MARKET_DATA_URL)
    assert_nil AppConfig.find_by(key: AppConfig::MARKET_DATA_TOKEN)
    assert_nil AppConfig.find_by(key: 'platform_connected_at')
    assert_nil AppConfig.find_by(key: 'proxy_binance')
    assert_nil AppConfig.find_by(key: 'proxy_custom_exchange')
    assert_equal 'keep-me', AppConfig.get('proxywithoutunderscore')
    assert_equal AppConfig::SYNC_STATUS_PENDING, AppConfig.setup_sync_status
  end

  test 'platform disconnect is admin-only and leaves configuration untouched' do
    AppConfig.set('platform_connected_at', Time.current.iso8601)
    AppConfig.set('proxy_binance', 'http://claimed-binance.example.com')
    member = create(:user, setup_completed: true)
    sign_in member

    delete settings_platform_connection_path, as: :turbo_stream

    assert_response :forbidden
    assert AppConfig.get('platform_connected_at').present?
    assert_equal 'http://claimed-binance.example.com', AppConfig.get('proxy_binance')
  end

  test 'platform disconnect preserves a separately selected CoinGecko feed and its key' do
    AppConfig.market_data_provider = MarketDataSettings::PROVIDER_COINGECKO
    AppConfig.market_data_url = 'https://claimed-data.example.com'
    AppConfig.market_data_token = 'dbi_claimed'
    AppConfig.coingecko_api_key = 'saved-key'
    AppConfig.set('platform_connected_at', Time.current.iso8601)
    AppConfig.set('proxy_binance', 'http://claimed-binance.example.com')

    delete settings_platform_connection_path, as: :turbo_stream

    assert_response :success
    assert_equal MarketDataSettings::PROVIDER_COINGECKO, AppConfig.market_data_provider
    assert_equal 'saved-key', AppConfig.coingecko_api_key
    assert_nil AppConfig.find_by(key: AppConfig::MARKET_DATA_URL)
    assert_nil AppConfig.find_by(key: AppConfig::MARKET_DATA_TOKEN)
    assert_nil AppConfig.find_by(key: 'platform_connected_at')
    assert_nil AppConfig.find_by(key: 'proxy_binance')
  end

  test 'platform connection copy exists in every locale' do
    settings_keys = %w[option code connect connected_with_proxies connected_without_proxies disconnect disconnected]
    setup_keys = %w[prompt code connect connected_with_proxies connected_without_proxies]

    I18n.available_locales.each do |locale|
      settings_keys.each do |key|
        assert I18n.exists?("settings.platform.#{key}", locale, fallback: false),
               "missing settings.platform.#{key} in #{locale}"
      end
      setup_keys.each do |key|
        assert I18n.exists?("setup.platform.#{key}", locale, fallback: false),
               "missing setup.platform.#{key} in #{locale}"
      end
    end
  end

  private

  def with_env(key, value)
    original = ENV[key]
    ENV[key] = value
    yield
  ensure
    original.nil? ? ENV.delete(key) : ENV[key] = original
  end
end
