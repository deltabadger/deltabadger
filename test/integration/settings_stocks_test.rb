require 'test_helper'

class SettingsStocksTest < ActionDispatch::IntegrationTest
  include Devise::Test::IntegrationHelpers

  setup do
    @admin = create(:user, admin: true, setup_completed: true)
    @exchange = create(:alpaca_exchange)
    sign_in @admin
  end

  test 'stocks widget shows OFF when not configured' do
    get settings_path
    assert_response :success
    assert_select 'turbo-frame#stocks_settings'
  end

  test 'enable stocks with valid API key saves config and creates ApiKey' do
    Clients::Alpaca.any_instance.stubs(:get_account).returns(
      Result::Success.new({ 'status' => 'ACTIVE' })
    )
    Exchange::SyncAlpacaAssetsJob.stubs(:perform_later)

    assert_difference '@admin.api_keys.count', 1 do
      patch settings_update_stocks_path, params: {
        alpaca_api_key: 'test_key',
        alpaca_api_secret: 'test_secret',
        alpaca_mode: 'paper'
      }
    end

    assert_equal 'test_key', AppConfig.get('alpaca_api_key')
    assert_equal 'test_secret', AppConfig.get('alpaca_api_secret')
    assert_equal 'paper', AppConfig.get('alpaca_mode')

    api_key = @admin.api_keys.find_by(exchange: @exchange)
    assert_equal 'test_key', api_key.key
    assert_equal 'test_secret', api_key.secret
    assert_equal 'paper', api_key.passphrase
    assert api_key.correct?
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

  test 'disconnect stocks clears settings and marks tickers unavailable' do
    AppConfig.set('alpaca_api_key', 'test_key')
    AppConfig.set('alpaca_api_secret', 'test_secret')
    AppConfig.set('alpaca_mode', 'paper')

    usd = Asset.find_or_create_by!(external_id: 'usd') do |a|
      a.symbol = 'USD'
      a.name = 'US Dollar'
      a.category = 'Fiat'
    end
    aapl = Asset.create!(external_id: 'alpaca_uuid-aapl', symbol: 'AAPL', name: 'Apple Inc', category: 'Stock')
    ExchangeAsset.create!(exchange: @exchange, asset: aapl)
    ExchangeAsset.create!(exchange: @exchange, asset: usd)
    Ticker.create!(exchange: @exchange, base_asset: aapl, quote_asset: usd, base: 'AAPL', quote: 'USD', ticker: 'AAPL',
                   minimum_base_size: 0, minimum_quote_size: 1, base_decimals: 9, quote_decimals: 2, price_decimals: 2)

    delete settings_disconnect_stocks_path

    assert_nil AppConfig.get('alpaca_api_key')
    assert_nil AppConfig.get('alpaca_api_secret')
    assert_nil AppConfig.get('alpaca_mode')
    refute Ticker.find_by(exchange: @exchange, base_asset: aapl).available?
  end
end
