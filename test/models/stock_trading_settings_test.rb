require 'test_helper'

class StockTradingSettingsTest < ActiveSupport::TestCase
  test 'inactive when there is no data API and no stock catalog' do
    refute StockTradingSettings.active?
    refute StockTradingSettings.deltabadger?
  end

  test 'active on hosted — the data API provides the catalog' do
    MarketDataSettings.stubs(:current_provider).returns(MarketDataSettings::PROVIDER_DELTABADGER)

    assert StockTradingSettings.active?
    assert StockTradingSettings.deltabadger?
  end

  test 'active when an available stock-venue ticker exists, even with no credential' do
    exchange = create(:alpaca_exchange)
    aapl = create(:asset, external_id: 'alpaca_uuid-aapl', symbol: 'AAPL', name: 'Apple Inc', category: 'Stock')
    usd = create(:asset, :usd)
    create(:ticker, exchange: exchange, base_asset: aapl, quote_asset: usd, available: true)

    refute AppConfig.alpaca_configured?
    assert StockTradingSettings.active?
    refute StockTradingSettings.deltabadger?
  end

  test 'unavailable stock-venue tickers do not activate' do
    exchange = create(:alpaca_exchange)
    aapl = create(:asset, external_id: 'alpaca_uuid-aapl', symbol: 'AAPL', name: 'Apple Inc', category: 'Stock')
    usd = create(:asset, :usd)
    create(:ticker, exchange: exchange, base_asset: aapl, quote_asset: usd, available: false)

    refute StockTradingSettings.active?
  end

  test 'available crypto tickers do not activate' do
    exchange = create(:binance_exchange)
    btc = create(:asset, :bitcoin)
    usd = create(:asset, :usd)
    create(:ticker, exchange: exchange, base_asset: btc, quote_asset: usd, available: true)

    refute StockTradingSettings.active?
  end

  test 'a credential alone (before the first sync) does not activate' do
    AppConfig.set('alpaca_api_key', 'k')
    AppConfig.set('alpaca_api_secret', 's')

    refute StockTradingSettings.active?
  end

  test 'ibkr is available only on hosted — the IBKR catalog is data-api served' do
    refute StockTradingSettings.ibkr_available?

    MarketDataSettings.stubs(:current_provider).returns(MarketDataSettings::PROVIDER_DELTABADGER)
    assert StockTradingSettings.ibkr_available?
  end

  test 'a self-hosted Alpaca catalog activates stocks but not IBKR' do
    exchange = create(:alpaca_exchange)
    aapl = create(:asset, external_id: 'alpaca_uuid-aapl', symbol: 'AAPL', name: 'Apple Inc', category: 'Stock')
    usd = create(:asset, :usd)
    create(:ticker, exchange: exchange, base_asset: aapl, quote_asset: usd, available: true)

    assert StockTradingSettings.active?
    refute StockTradingSettings.ibkr_available?
  end
end
