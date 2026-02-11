require 'test_helper'

class MarketDataImportTickersTest < ActiveSupport::TestCase
  setup do
    @exchange = create(:binance_exchange)
    @btc = create(:asset, :bitcoin)
    @usd = create(:asset, :usd)
    @eth = create(:asset, :ethereum)
  end

  def ticker_data(base_ext_id:, quote_ext_id:, base:, quote:, ticker:)
    {
      'base_external_id' => base_ext_id,
      'quote_external_id' => quote_ext_id,
      'base' => base,
      'quote' => quote,
      'ticker' => ticker,
      'minimum_base_size' => '0.00001',
      'minimum_quote_size' => '10',
      'maximum_base_size' => '10000',
      'maximum_quote_size' => '1000000',
      'base_decimals' => 8,
      'quote_decimals' => 2,
      'price_decimals' => 2
    }
  end

  test 'creates tickers and exchange assets from data' do
    data = [
      ticker_data(base_ext_id: 'bitcoin', quote_ext_id: 'usd', base: 'BTC', quote: 'USD', ticker: 'BTCUSD')
    ]

    MarketData.import_tickers!(@exchange, data)

    assert_equal 1, @exchange.tickers.count
    t = @exchange.tickers.first
    assert_equal 'BTC', t.base
    assert_equal 'USD', t.quote
    assert_equal 'BTCUSD', t.ticker
    assert_equal @btc.id, t.base_asset_id
    assert_equal @usd.id, t.quote_asset_id
    assert_equal BigDecimal('0.00001'), t.minimum_base_size
    assert_equal BigDecimal('10'), t.minimum_quote_size
    assert t.available
  end

  test 'creates exchange assets for both base and quote' do
    data = [
      ticker_data(base_ext_id: 'bitcoin', quote_ext_id: 'usd', base: 'BTC', quote: 'USD', ticker: 'BTCUSD')
    ]

    MarketData.import_tickers!(@exchange, data)

    assert_equal 2, @exchange.exchange_assets.count
    asset_ids = @exchange.exchange_assets.pluck(:asset_id)
    assert_includes asset_ids, @btc.id
    assert_includes asset_ids, @usd.id
    assert @exchange.exchange_assets.all?(&:available)
  end

  test 'handles multiple tickers in one call' do
    data = [
      ticker_data(base_ext_id: 'bitcoin', quote_ext_id: 'usd', base: 'BTC', quote: 'USD', ticker: 'BTCUSD'),
      ticker_data(base_ext_id: 'ethereum', quote_ext_id: 'usd', base: 'ETH', quote: 'USD', ticker: 'ETHUSD')
    ]

    MarketData.import_tickers!(@exchange, data)

    assert_equal 2, @exchange.tickers.count
    assert_equal 3, @exchange.exchange_assets.count
  end

  test 'skips tickers with unknown assets' do
    data = [
      ticker_data(base_ext_id: 'bitcoin', quote_ext_id: 'usd', base: 'BTC', quote: 'USD', ticker: 'BTCUSD'),
      ticker_data(base_ext_id: 'dogecoin', quote_ext_id: 'usd', base: 'DOGE', quote: 'USD', ticker: 'DOGEUSD')
    ]

    MarketData.import_tickers!(@exchange, data)

    assert_equal 1, @exchange.tickers.count
    assert_equal 'BTC', @exchange.tickers.first.base
  end

  test 'upserts existing tickers without duplicates' do
    data = [
      ticker_data(base_ext_id: 'bitcoin', quote_ext_id: 'usd', base: 'BTC', quote: 'USD', ticker: 'BTCUSD')
    ]

    MarketData.import_tickers!(@exchange, data)
    assert_equal 1, @exchange.tickers.count

    # Run again — should update, not duplicate
    MarketData.import_tickers!(@exchange, data)
    assert_equal 1, @exchange.tickers.count
  end

  test 'updates ticker attributes on re-import' do
    data = [
      ticker_data(base_ext_id: 'bitcoin', quote_ext_id: 'usd', base: 'BTC', quote: 'USD', ticker: 'BTCUSD')
    ]

    MarketData.import_tickers!(@exchange, data)

    data.first['minimum_base_size'] = '0.001'
    MarketData.import_tickers!(@exchange, data)

    assert_equal BigDecimal('0.001'), @exchange.tickers.first.minimum_base_size
  end

  test 'handles nil maximum sizes' do
    data = [
      ticker_data(base_ext_id: 'bitcoin', quote_ext_id: 'usd', base: 'BTC', quote: 'USD', ticker: 'BTCUSD')
        .merge('maximum_base_size' => nil, 'maximum_quote_size' => nil)
    ]

    MarketData.import_tickers!(@exchange, data)

    t = @exchange.tickers.first
    assert_nil t.maximum_base_size
    assert_nil t.maximum_quote_size
  end

  test 'does nothing with blank data' do
    MarketData.import_tickers!(@exchange, nil)
    MarketData.import_tickers!(@exchange, [])

    assert_equal 0, @exchange.tickers.count
    assert_equal 0, @exchange.exchange_assets.count
  end

  test 'upserts when existing ticker has different symbol strings but same asset IDs' do
    # Simulate upgrade scenario: Kraken used XDG for Dogecoin, now uses DOGE
    doge = create(:asset, external_id: 'dogecoin', symbol: 'DOGE', name: 'Dogecoin', category: 'Cryptocurrency')
    create(:exchange_asset, exchange: @exchange, asset: doge)
    create(:exchange_asset, exchange: @exchange, asset: @usd)
    Ticker.create!(
      exchange: @exchange, base: 'XDG', quote: 'USD', ticker: 'XDGUSD',
      base_asset_id: doge.id, quote_asset_id: @usd.id,
      minimum_base_size: 1, minimum_quote_size: 10, base_decimals: 8, quote_decimals: 2, price_decimals: 2
    )

    data = [
      ticker_data(base_ext_id: 'dogecoin', quote_ext_id: 'usd', base: 'DOGE', quote: 'USD', ticker: 'DOGEUSD')
    ]

    MarketData.import_tickers!(@exchange, data)

    assert_equal 1, @exchange.tickers.count
    t = @exchange.tickers.first
    assert_equal 'DOGE', t.base
    assert_equal 'USD', t.quote
    assert_equal 'DOGEUSD', t.ticker
    assert_equal doge.id, t.base_asset_id
  end

  test 'upserts when existing ticker has different ticker string but same asset IDs' do
    create(:exchange_asset, exchange: @exchange, asset: @btc)
    create(:exchange_asset, exchange: @exchange, asset: @usd)
    Ticker.create!(
      exchange: @exchange, base: 'BTC', quote: 'USD', ticker: 'XBTUSD',
      base_asset_id: @btc.id, quote_asset_id: @usd.id,
      minimum_base_size: 0.00001, minimum_quote_size: 10, base_decimals: 8, quote_decimals: 2, price_decimals: 2
    )

    data = [
      ticker_data(base_ext_id: 'bitcoin', quote_ext_id: 'usd', base: 'BTC', quote: 'USD', ticker: 'BTCUSD')
    ]

    MarketData.import_tickers!(@exchange, data)

    assert_equal 1, @exchange.tickers.count
    t = @exchange.tickers.first
    assert_equal 'BTCUSD', t.ticker
    assert_equal @btc.id, t.base_asset_id
  end

  test 'upserts exchange assets without duplicates' do
    data = [
      ticker_data(base_ext_id: 'bitcoin', quote_ext_id: 'usd', base: 'BTC', quote: 'USD', ticker: 'BTCUSD')
    ]

    MarketData.import_tickers!(@exchange, data)
    assert_equal 2, @exchange.exchange_assets.count

    MarketData.import_tickers!(@exchange, data)
    assert_equal 2, @exchange.exchange_assets.count
  end
end
