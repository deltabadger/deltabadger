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
    assert t.trading_enabled, 'defaults trading_enabled to true when payload omits it'
  end

  test 'imports trading_enabled from the payload' do
    data = [
      ticker_data(base_ext_id: 'bitcoin', quote_ext_id: 'usd', base: 'BTC', quote: 'USD', ticker: 'BTCUSD')
        .merge('trading_enabled' => false)
    ]

    MarketData.import_tickers!(@exchange, data)

    t = @exchange.tickers.find_by(ticker: 'BTCUSD')
    assert t.available, 'listed pairs stay available'
    assert_not t.trading_enabled, 'disabled pair imports trading_enabled: false'
  end

  # Regression: a ticker-string remap (the feed reassigns an existing ticker string to a
  # different asset pair) must reconcile without hitting the [exchange_id, ticker] unique
  # index. This crashed db:seed and crash-looped containers on boot.
  test 'reconciles a ticker string reassigned to a different asset pair without crashing' do
    # A: same asset pair as incoming, old ticker string
    a = create(:ticker, exchange: @exchange, base_asset: @btc, quote_asset: @usd,
                        base: 'BTC', quote: 'USD', ticker: 'BTCUSD')
    # B: holds the incoming ticker string, but for a different asset pair
    b = create(:ticker, exchange: @exchange, base_asset: @eth, quote_asset: @usd,
                        base: 'ETH', quote: 'USD', ticker: 'XBTUSD')

    # incoming wants A's pair (BTC/USD) renamed to B's ticker string ('XBTUSD')
    data = [ticker_data(base_ext_id: 'bitcoin', quote_ext_id: 'usd', base: 'BTC', quote: 'USD', ticker: 'XBTUSD')]

    assert_nothing_raised do
      MarketData.import_tickers!(@exchange, data)
    end

    assert_equal 'XBTUSD', a.reload.ticker, "A's pair should be renamed to the incoming ticker string"

    # Tickers are undeletable (and referenced by bot_index_assets FK), so the stale holder is
    # preserved — moved out of the unique namespace and marked unavailable, not deleted.
    assert Ticker.exists?(b.id), 'stale holder must be preserved (tickers are undeletable)'
    b.reload
    assert_not b.available?, 'stale holder should be marked unavailable'
    assert_not_equal 'XBTUSD', b.ticker, 'stale holder must no longer occupy the reassigned ticker string'
    assert_equal 1, @exchange.tickers.available.where(ticker: 'XBTUSD').count, 'exactly one available ticker owns the string'
    assert_equal @btc.id, @exchange.tickers.find_by(ticker: 'XBTUSD').base_asset_id
  end

  # Regression: the feed reassigns a base/quote SYMBOL pair to a different asset pair. This must
  # reconcile without hitting the [exchange_id, base, quote] unique index (the symbol index — a
  # different secondary index than [exchange_id, ticker]). Surfaced via Exchange::SyncTickersAndAssetsJob.
  test 'reconciles base/quote symbols reassigned to a different asset pair without crashing' do
    # A: holds the [BTC, USD] symbol pair for the bitcoin/usd asset pair
    a = create(:ticker, exchange: @exchange, base_asset: @btc, quote_asset: @usd,
                        base: 'BTC', quote: 'USD', ticker: 'BTCUSD')
    # B: a different asset pair (eth/usd) with old symbols and its own ticker string
    b = create(:ticker, exchange: @exchange, base_asset: @eth, quote_asset: @usd,
                        base: 'ETHOLD', quote: 'USD', ticker: 'ETHUSD')

    # incoming: eth/usd now reports base symbol 'BTC' — collides with A's [BTC, USD], different asset pair
    data = [ticker_data(base_ext_id: 'ethereum', quote_ext_id: 'usd', base: 'BTC', quote: 'USD', ticker: 'ETHUSD')]

    assert_nothing_raised do
      MarketData.import_tickers!(@exchange, data)
    end

    # eth/usd now owns [BTC, USD]; the bitcoin/usd holder was moved out of the way, preserved-but-unavailable
    assert_equal %w[BTC USD], [b.reload.base, b.quote]
    assert Ticker.exists?(a.id), 'stale base/quote holder must be preserved (tickers are undeletable)'
    assert_not a.reload.available?, 'stale holder should be marked unavailable'
    assert_not_equal %w[BTC USD], [a.base, a.quote], 'stale holder must no longer occupy the [base, quote] pair'
  end

  # Regression for the 2.9.2 upgrade path: that release tombstoned `ticker` but NOT `base`, so a
  # deployed row can have a tombstoned ticker while still owning a real [base, quote] pair. The
  # reconcile must still free that [base, quote] slot (not skip the row just because its ticker is
  # already tombstoned).
  test 'tombstones base even when the stale holder already has a tombstoned ticker' do
    # R: 2.9.2 intermediate state — ticker tombstoned + unavailable, but base still 'BTC'
    r = create(:ticker, exchange: @exchange, base_asset: @btc, quote_asset: @usd,
                        base: 'BTC', quote: 'USD', ticker: '__stale_999_BTCUSD', available: false)
    # B: a different asset pair whose incoming row will claim [BTC, USD]
    b = create(:ticker, exchange: @exchange, base_asset: @eth, quote_asset: @usd,
                        base: 'ETHOLD', quote: 'USD', ticker: 'ETHUSD')

    data = [ticker_data(base_ext_id: 'ethereum', quote_ext_id: 'usd', base: 'BTC', quote: 'USD', ticker: 'ETHUSD')]

    assert_nothing_raised do
      MarketData.import_tickers!(@exchange, data)
    end

    assert_equal %w[BTC USD], [b.reload.base, b.quote]
    assert Ticker.exists?(r.id)
    assert_not_equal 'BTC', r.reload.base, 'the already-ticker-tombstoned row must also have its base freed'
    assert_not r.available?
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
