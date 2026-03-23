require 'test_helper'

class MarketDataSettingsTest < ActiveSupport::TestCase
  teardown do
    ENV.delete('MARKET_DATA_URL')
  end

  test 'current_provider returns deltabadger when MARKET_DATA_URL is set regardless of db setting' do
    ENV['MARKET_DATA_URL'] = 'http://data-api:3000'
    AppConfig.market_data_provider = MarketDataSettings::PROVIDER_COINGECKO

    assert_equal MarketDataSettings::PROVIDER_DELTABADGER, MarketDataSettings.current_provider
    assert MarketDataSettings.deltabadger?
    refute MarketDataSettings.coingecko?
  end

  test 'current_provider returns db setting when MARKET_DATA_URL is not set' do
    ENV.delete('MARKET_DATA_URL')
    AppConfig.market_data_provider = MarketDataSettings::PROVIDER_COINGECKO

    assert_equal MarketDataSettings::PROVIDER_COINGECKO, MarketDataSettings.current_provider
    assert MarketDataSettings.coingecko?
    refute MarketDataSettings.deltabadger?
  end

  test 'current_provider returns nil when MARKET_DATA_URL is not set and no db setting' do
    ENV.delete('MARKET_DATA_URL')
    AppConfig.market_data_provider = nil

    assert_nil MarketDataSettings.current_provider
    refute MarketDataSettings.configured?
  end
end
