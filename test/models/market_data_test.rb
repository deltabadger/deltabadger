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

class MarketDataStockColorsTest < ActiveSupport::TestCase
  test 'returns {} when not in hosted (deltabadger) mode' do
    MarketDataSettings.stubs(:deltabadger?).returns(false)
    assert_equal({}, MarketData.stock_colors)
  end

  test 'returns the data hash from the client in hosted mode' do
    MarketDataSettings.stubs(:deltabadger?).returns(true)
    fake = mock
    fake.stubs(:get_stock_colors).returns(
      Result::Success.new('metadata' => { 'count' => 1 }, 'data' => { 'QQQM' => '#000AD2' })
    )
    MarketData.stubs(:client).returns(fake)

    assert_equal({ 'QQQM' => '#000AD2' }, MarketData.stock_colors)
  end

  test 'returns {} when the client fails (best-effort, never raises)' do
    MarketDataSettings.stubs(:deltabadger?).returns(true)
    fake = mock
    fake.stubs(:get_stock_colors).returns(Result::Failure.new('boom'))
    MarketData.stubs(:client).returns(fake)

    assert_equal({}, MarketData.stock_colors)
  end
end

class MarketDataSyncStocksFromDeltabadgerTest < ActiveSupport::TestCase
  setup do
    MarketDataSettings.stubs(:deltabadger?).returns(true)
    @fake = mock
    MarketData.stubs(:client).returns(@fake)
  end

  def stock_row(external_id:, symbol:, type: 'stock', color: nil)
    {
      'external_id' => external_id, 'symbol' => symbol, 'name' => symbol, 'type' => type,
      'color' => color, 'category' => (type == 'stock' ? 'Common Stock' : 'ETF'),
      'identifiers' => [{ 'scheme' => 'alpaca', 'value' => "us_equity:#{symbol}" }]
    }
  end

  test 'upserts stock + etf rows as category=Stock keyed by external_id' do
    @fake.stubs(:get_stocks).returns(Result::Success.new(
                                       'metadata' => { 'count' => 2 },
                                       'data' => [stock_row(external_id: 'AAPL.US', symbol: 'AAPL'),
                                                  stock_row(external_id: 'SPY.US', symbol: 'SPY', type: 'etf')]
                                     ))

    assert_difference 'Asset.count', 2 do
      MarketData.sync_stocks_from_deltabadger!
    end

    aapl = Asset.find_by(external_id: 'AAPL.US')
    spy = Asset.find_by(external_id: 'SPY.US')
    assert_equal 'Stock', aapl.category, 'stock asset_type maps to category=Stock'
    assert_equal 'Stock', spy.category, 'etf asset_type also maps to Stock (R5-F1: preserves all category==Stock gates)'
  end

  test 'applies color from payload' do
    @fake.stubs(:get_stocks).returns(Result::Success.new(
                                       'metadata' => { 'count' => 1 },
                                       'data' => [stock_row(external_id: 'AAPL.US', symbol: 'AAPL', color: '#FF0000')]
                                     ))

    MarketData.sync_stocks_from_deltabadger!
    assert_equal '#FF0000', Asset.find_by(external_id: 'AAPL.US').color
  end

  test 'is idempotent — second run does not duplicate' do
    @fake.stubs(:get_stocks).returns(Result::Success.new(
                                       'metadata' => { 'count' => 1 },
                                       'data' => [stock_row(external_id: 'AAPL.US', symbol: 'AAPL')]
                                     ))

    MarketData.sync_stocks_from_deltabadger!
    assert_no_difference 'Asset.count' do
      MarketData.sync_stocks_from_deltabadger!
    end
  end

  test 'skips unknown asset_type values and logs (never written through raw)' do
    @fake.stubs(:get_stocks).returns(Result::Success.new(
                                       'metadata' => { 'count' => 3 },
                                       'data' => [
                                         stock_row(external_id: 'AAPL.US', symbol: 'AAPL'),
                                         stock_row(external_id: 'WAT.US', symbol: 'WAT', type: 'preferred_stock'),
                                         stock_row(external_id: 'SPY.US', symbol: 'SPY', type: 'etf')
                                       ]
                                     ))

    MarketData.sync_stocks_from_deltabadger!
    assert_equal %w[AAPL.US SPY.US].sort, Asset.where(category: 'Stock').pluck(:external_id).sort
    assert_nil Asset.find_by(external_id: 'WAT.US'), 'unknown asset_type must not be written through raw'
  end

  test 'returns Result::Failure and writes nothing when client fails' do
    @fake.stubs(:get_stocks).returns(Result::Failure.new('boom'))
    assert_no_difference 'Asset.count' do
      result = MarketData.sync_stocks_from_deltabadger!
      assert_predicate result, :failure?
    end
  end
end

class MarketDataSyncAlpacaListingsFromDeltabadgerTest < ActiveSupport::TestCase
  setup do
    MarketDataSettings.stubs(:deltabadger?).returns(true)
    @alpaca = create(:alpaca_exchange)
    @aapl = Asset.create!(external_id: 'AAPL.US', symbol: 'AAPL', name: 'Apple', category: 'Stock')
    @spy = Asset.create!(external_id: 'SPY.US', symbol: 'SPY', name: 'SPDR S&P 500 ETF', category: 'Stock')
    @fake = mock
    MarketData.stubs(:client).returns(@fake)
  end

  def listing_row(base_ext:, symbol:, fractionable: true, quote_ext: 'USD.FOREX', listing_id: nil)
    {
      'listing_id' => listing_id || "NASDAQ:#{symbol}",
      'base' => symbol, 'quote' => 'USD', 'ticker' => symbol,
      'base_external_id' => base_ext, 'quote_external_id' => quote_ext,
      'fractionable' => fractionable
    }
  end

  test "guarantees the local 'usd' Asset row exists (Invariant A)" do
    assert_nil Asset.find_by(external_id: 'usd')
    @fake.stubs(:get_alpaca_listings).returns(Result::Success.new(
                                                'metadata' => { 'count' => 1 },
                                                'data' => [listing_row(base_ext: 'AAPL.US', symbol: 'AAPL')]
                                              ))

    MarketData.sync_alpaca_listings_from_deltabadger!

    usd = Asset.find_by(external_id: 'usd')
    assert usd.present?, "wizard requires 'usd' row at pick_buyable_assets_controller.rb:43"
    assert_equal 'USD', usd.symbol
    assert_equal 'Fiat', usd.category
  end

  test "every created Ticker's quote_asset is the local 'usd' row regardless of payload quote_external_id (Invariant B)" do
    @fake.stubs(:get_alpaca_listings).returns(Result::Success.new(
                                                'metadata' => { 'count' => 2 },
                                                'data' => [
                                                  listing_row(base_ext: 'AAPL.US', symbol: 'AAPL', quote_ext: 'USD.FOREX'),
                                                  listing_row(base_ext: 'SPY.US', symbol: 'SPY', quote_ext: 'usd')
                                                ]
                                              ))

    MarketData.sync_alpaca_listings_from_deltabadger!

    @alpaca.tickers.reload.each do |t|
      assert_equal 'usd', t.quote_asset.external_id,
                   'every Alpaca ticker must point at the local usd row (R5-F3 / R4-F2)'
    end
  end

  test "end-to-end picker wiring: 'usd' row + Alpaca ticker found by (base, quote_asset)" do
    @fake.stubs(:get_alpaca_listings).returns(Result::Success.new(
                                                'metadata' => { 'count' => 1 },
                                                'data' => [listing_row(base_ext: 'AAPL.US', symbol: 'AAPL')]
                                              ))

    MarketData.sync_alpaca_listings_from_deltabadger!

    usd = Asset.find_by(external_id: 'usd')
    # Same shape as app/models/bots/dca_single_asset.rb:226
    assert @alpaca.tickers.available.trading_enabled.exists?(base_asset: @aapl, quote_asset: usd),
           "wizard's (base_asset, quote_asset) lookup must find the seeded ticker"
  end

  test 'injects stock trading defaults (decimals + min/max sizes) since data-api MarketListing has none' do
    @fake.stubs(:get_alpaca_listings).returns(Result::Success.new(
                                                'metadata' => { 'count' => 1 },
                                                'data' => [listing_row(base_ext: 'AAPL.US', symbol: 'AAPL')]
                                              ))

    MarketData.sync_alpaca_listings_from_deltabadger!

    ticker = @alpaca.tickers.find_by(base_asset: @aapl)
    assert_equal 9, ticker.base_decimals
    assert_equal 2, ticker.quote_decimals
    assert_equal 2, ticker.price_decimals
    assert_equal BigDecimal('0.000000001'), ticker.minimum_base_size
    assert_equal BigDecimal('1'), ticker.minimum_quote_size
    assert_equal BigDecimal('100000'), ticker.maximum_base_size
    assert_equal BigDecimal('10000000'), ticker.maximum_quote_size
    assert ticker.available?
  end

  test 'defensively drops fractionable: false rows even if data-api sends them' do
    @fake.stubs(:get_alpaca_listings).returns(Result::Success.new(
                                                'metadata' => { 'count' => 2 },
                                                'data' => [
                                                  listing_row(base_ext: 'AAPL.US', symbol: 'AAPL', fractionable: true),
                                                  listing_row(base_ext: 'SPY.US', symbol: 'SPY', fractionable: false)
                                                ]
                                              ))

    MarketData.sync_alpaca_listings_from_deltabadger!
    assert @alpaca.tickers.where(base_asset: @aapl).exists?
    assert_not @alpaca.tickers.where(base_asset: @spy).exists?, 'non-fractionable must be dropped client-side too (R7-F1)'
  end

  test 'sweeps stale Alpaca tickers (whose base_asset_id is absent from incoming) to available: false' do
    @fake.stubs(:get_alpaca_listings).returns(Result::Success.new(
                                                'metadata' => { 'count' => 2 },
                                                'data' => [listing_row(base_ext: 'AAPL.US', symbol: 'AAPL'),
                                                           listing_row(base_ext: 'SPY.US', symbol: 'SPY')]
                                              ))
    MarketData.sync_alpaca_listings_from_deltabadger!

    # Second sync drops SPY
    @fake.stubs(:get_alpaca_listings).returns(Result::Success.new(
                                                'metadata' => { 'count' => 1 },
                                                'data' => [listing_row(base_ext: 'AAPL.US', symbol: 'AAPL')]
                                              ))
    MarketData.sync_alpaca_listings_from_deltabadger!

    spy_ticker = @alpaca.tickers.find_by(base_asset: @spy)
    assert spy_ticker.present?, 'tickers are undeletable — must remain in the table'
    assert_not spy_ticker.available?, 'stale ticker must be swept to available: false'
    assert @alpaca.tickers.find_by(base_asset: @aapl).available?, 'live ticker stays available'
  end

  test 'empty payload guard: does NOT wipe existing tickers (R7-F3)' do
    @fake.stubs(:get_alpaca_listings).returns(Result::Success.new(
                                                'metadata' => { 'count' => 1 },
                                                'data' => [listing_row(base_ext: 'AAPL.US', symbol: 'AAPL')]
                                              ))
    MarketData.sync_alpaca_listings_from_deltabadger!

    # Now a buggy/partial payload returns empty
    @fake.stubs(:get_alpaca_listings).returns(Result::Success.new(
                                                'metadata' => { 'count' => 0 }, 'data' => []
                                              ))
    MarketData.sync_alpaca_listings_from_deltabadger!

    assert @alpaca.tickers.find_by(base_asset: @aapl).available?,
           'empty incoming payload must not flip every Alpaca ticker to unavailable'
  end

  test 'returns Result::Failure when client fails, no DB writes' do
    @fake.stubs(:get_alpaca_listings).returns(Result::Failure.new('boom'))
    assert_no_difference ['Ticker.count', 'Asset.count'] do
      result = MarketData.sync_alpaca_listings_from_deltabadger!
      assert_predicate result, :failure?
    end
  end
end

class MarketDataBackfillCanonicalStockExternalIdsTest < ActiveSupport::TestCase
  setup do
    MarketDataSettings.stubs(:deltabadger?).returns(true)
    begin
      AppConfig.delete('stock_canonical_backfill_completed_at')
    rescue StandardError
      nil
    end
    @fake = mock
    MarketData.stubs(:client).returns(@fake)
  end

  def stocks_payload(rows)
    Result::Success.new('metadata' => { 'count' => rows.size }, 'data' => rows)
  end

  def stock_with_alpaca_id(external_id:, symbol:)
    { 'external_id' => external_id, 'symbol' => symbol, 'name' => symbol, 'type' => 'stock',
      'identifiers' => [{ 'scheme' => 'alpaca', 'value' => "us_equity:#{symbol}" }] }
  end

  test 'no-op in free mode (open-source containers skip entirely)' do
    MarketDataSettings.stubs(:deltabadger?).returns(false)
    legacy = Asset.create!(external_id: 'alpaca_uuid-aapl', symbol: 'AAPL', name: 'Apple', category: 'Stock')

    assert_no_difference 'Asset.count' do
      MarketData.backfill_canonical_stock_external_ids!
    end
    assert_equal 'alpaca_uuid-aapl', legacy.reload.external_id
    assert_nil AppConfig.get('stock_canonical_backfill_completed_at'),
               'flag must not be set in free mode'
  end

  test 'rewrites legacy alpaca_<uuid> external_ids to canonical, preserving id (FK-safe)' do
    legacy = Asset.create!(external_id: 'alpaca_uuid-aapl', symbol: 'AAPL', name: 'Apple', category: 'Stock')
    original_id = legacy.id
    @fake.stubs(:get_stocks).returns(stocks_payload([stock_with_alpaca_id(external_id: 'AAPL.US', symbol: 'AAPL')]))

    MarketData.backfill_canonical_stock_external_ids!

    legacy.reload
    assert_equal 'AAPL.US', legacy.external_id, 'external_id must be rewritten to canonical'
    assert_equal original_id, legacy.id, 'id must be preserved so FKs remain valid'
  end

  test 'leaves unmatched legacy rows alone (logged + counted, not deleted)' do
    legacy = Asset.create!(external_id: 'alpaca_uuid-zzz', symbol: 'ZZZ', name: 'Mystery', category: 'Stock')
    @fake.stubs(:get_stocks).returns(stocks_payload([stock_with_alpaca_id(external_id: 'AAPL.US', symbol: 'AAPL')]))

    MarketData.backfill_canonical_stock_external_ids!
    assert_equal 'alpaca_uuid-zzz', legacy.reload.external_id, 'unmatched legacy row left untouched'
  end

  test 'defensive skip: if canonical row already exists, leave legacy alone (no unique-index collision)' do
    legacy = Asset.create!(external_id: 'alpaca_uuid-aapl', symbol: 'AAPL', name: 'Apple', category: 'Stock')
    Asset.create!(external_id: 'AAPL.US', symbol: 'AAPL', name: 'Apple', category: 'Stock')
    @fake.stubs(:get_stocks).returns(stocks_payload([stock_with_alpaca_id(external_id: 'AAPL.US', symbol: 'AAPL')]))

    assert_nothing_raised do
      MarketData.backfill_canonical_stock_external_ids!
    end
    assert_equal 'alpaca_uuid-aapl', legacy.reload.external_id, 'legacy left alone; no collision'
  end

  test 'sets the completed_at flag on success' do
    @fake.stubs(:get_stocks).returns(stocks_payload([stock_with_alpaca_id(external_id: 'AAPL.US', symbol: 'AAPL')]))
    MarketData.backfill_canonical_stock_external_ids!
    assert AppConfig.get('stock_canonical_backfill_completed_at').present?, 'flag must be set'
  end

  test 'idempotent: second invocation is a no-op once the flag is set' do
    legacy = Asset.create!(external_id: 'alpaca_uuid-aapl', symbol: 'AAPL', name: 'Apple', category: 'Stock')
    @fake.stubs(:get_stocks).returns(stocks_payload([stock_with_alpaca_id(external_id: 'AAPL.US', symbol: 'AAPL')]))
    MarketData.backfill_canonical_stock_external_ids!

    # Simulate manual external_id change after the fact; second run must NOT touch anything.
    legacy.update_columns(external_id: 'manually-set')
    @fake.expects(:get_stocks).never
    MarketData.backfill_canonical_stock_external_ids!
    assert_equal 'manually-set', legacy.reload.external_id
  end

  test 'does NOT set the flag when client returns Result::Failure (so the next tick retries)' do
    Asset.create!(external_id: 'alpaca_uuid-aapl', symbol: 'AAPL', name: 'Apple', category: 'Stock')
    @fake.stubs(:get_stocks).returns(Result::Failure.new('data-api unreachable'))

    MarketData.backfill_canonical_stock_external_ids!
    assert_nil AppConfig.get('stock_canonical_backfill_completed_at'),
               'failed fetch must leave the flag unset so the next sync-job tick retries'
  end

  test 'does NOT set the flag when payload carries no alpaca-scheme identifiers' do
    Asset.create!(external_id: 'alpaca_uuid-aapl', symbol: 'AAPL', name: 'Apple', category: 'Stock')
    # All rows present but none has the alpaca identifier scheme (treat as "data-api not ready").
    @fake.stubs(:get_stocks).returns(stocks_payload([
                                                      { 'external_id' => 'AAPL.US', 'symbol' => 'AAPL', 'name' => 'Apple', 'type' => 'stock',
                                                        'identifiers' => [{ 'scheme' => 'eodhd', 'value' => 'AAPL.US' }] }
                                                    ]))

    MarketData.backfill_canonical_stock_external_ids!
    assert_nil AppConfig.get('stock_canonical_backfill_completed_at')
  end

  test 'ignores non-Stock alpaca_<uuid> rows (only sweeps category=Stock legacy)' do
    crypto = Asset.create!(external_id: 'alpaca_legacy_crypto', symbol: 'XYZ', name: 'XYZ', category: 'Cryptocurrency')
    @fake.stubs(:get_stocks).returns(stocks_payload([stock_with_alpaca_id(external_id: 'AAPL.US', symbol: 'XYZ')]))

    MarketData.backfill_canonical_stock_external_ids!
    assert_equal 'alpaca_legacy_crypto', crypto.reload.external_id, 'non-stock legacy rows must not be touched'
  end

  # == Ambiguity guard (post-incident 2026-05-28) =========================================
  # When data-api accumulates stale alpaca identifiers (the IBIT/LDRC case: one canonical
  # asset carrying both us_equity:IBIT AND us_equity:LDRC), the symbol->canonical map can
  # silently send the wrong canonical to a legacy alpaca_<uuid> row. Guard: exclude any
  # symbol that matches EITHER condition (logical OR):
  #   - symbol resolves to >1 distinct canonical external_id across the payload, OR
  #   - the canonical that symbol points to carries >1 alpaca identifier of its own.

  test 'ambiguity guard: symbol resolving to multiple canonicals is excluded from rewrite' do
    legacy = Asset.create!(external_id: 'alpaca_uuid-ibit', symbol: 'IBIT', name: 'Mystery', category: 'Stock')
    # Two distinct canonicals each claim us_equity:IBIT (simulated data-api regression).
    @fake.stubs(:get_stocks).returns(stocks_payload([
                                                      { 'external_id' => 'IBIT.US', 'symbol' => 'IBIT', 'name' => 'iShares BTC', 'type' => 'etf',
                                                        'identifiers' => [{ 'scheme' => 'alpaca', 'value' => 'us_equity:IBIT' }] },
                                                      { 'external_id' => 'LDRC.US', 'symbol' => 'LDRC', 'name' => 'iShares iBonds', 'type' => 'etf',
                                                        'identifiers' => [{ 'scheme' => 'alpaca', 'value' => 'us_equity:IBIT' }] }
                                                    ]))

    MarketData.backfill_canonical_stock_external_ids!

    assert_equal 'alpaca_uuid-ibit', legacy.reload.external_id, 'ambiguous symbol must NOT be rewritten'
  end

  test 'ambiguity guard: canonical with multiple alpaca identifiers excludes all of its symbols' do
    # The real-world IBIT/LDRC shape: ONE canonical asset accumulated TWO alpaca identifiers
    # because data-api's ensure_identifiers never removes superseded entries when a security
    # renames at EODHD/Alpaca for the same ISIN.
    legacy_ibit = Asset.create!(external_id: 'alpaca_uuid-ibit', symbol: 'IBIT', name: 'A', category: 'Stock')
    legacy_ldrc = Asset.create!(external_id: 'alpaca_uuid-ldrc', symbol: 'LDRC', name: 'B', category: 'Stock')

    @fake.stubs(:get_stocks).returns(stocks_payload([
                                                      { 'external_id' => 'LDRC.US', 'symbol' => 'LDRC', 'name' => 'iShares iBonds', 'type' => 'etf',
                                                        'identifiers' => [
                                                          { 'scheme' => 'alpaca', 'value' => 'us_equity:IBIT' },
                                                          { 'scheme' => 'alpaca', 'value' => 'us_equity:LDRC' }
                                                        ] }
                                                    ]))

    MarketData.backfill_canonical_stock_external_ids!

    assert_equal 'alpaca_uuid-ibit', legacy_ibit.reload.external_id,
                 'IBIT must NOT be rewritten — its canonical carries a stale alpaca identifier'
    assert_equal 'alpaca_uuid-ldrc', legacy_ldrc.reload.external_id,
                 'LDRC must NOT be rewritten either — same canonical carries multiple alpaca identifiers'
  end

  test 'ambiguity guard: previously-excluded legacy rows get reconsidered when data-api is later cleaned up' do
    Asset.create!(external_id: 'alpaca_uuid-ibit', symbol: 'IBIT', name: 'A', category: 'Stock')

    # First run: LDRC.US carries both alpaca identifiers — IBIT excluded.
    @fake.stubs(:get_stocks).returns(stocks_payload([
                                                      { 'external_id' => 'LDRC.US', 'symbol' => 'LDRC',
                                                        'name' => 'iShares iBonds', 'type' => 'etf',
                                                        'identifiers' => [
                                                          { 'scheme' => 'alpaca', 'value' => 'us_equity:IBIT' },
                                                          { 'scheme' => 'alpaca', 'value' => 'us_equity:LDRC' }
                                                        ] }
                                                    ]))
    MarketData.backfill_canonical_stock_external_ids!
    assert_equal 'alpaca_uuid-ibit', Asset.find_by(symbol: 'IBIT').external_id

    # Second run AFTER data-api cleanup: IBIT.US is now its own canonical, no stale alpaca id.
    @fake.stubs(:get_stocks).returns(stocks_payload([
                                                      stock_with_alpaca_id(external_id: 'IBIT.US', symbol: 'IBIT'),
                                                      stock_with_alpaca_id(external_id: 'LDRC.US', symbol: 'LDRC')
                                                    ]))
    MarketData.backfill_canonical_stock_external_ids!
    assert_equal 'IBIT.US', Asset.find_by(symbol: 'IBIT').external_id,
                 'backfill must re-run and rewrite previously-excluded legacy row when data-api is clean'
  end

  test 'self-pacing: no legacy rows + no flag = sets flag and skips data-api fetch' do
    # Containers that never had Alpaca configured have zero legacy rows from day one.
    # Backfill must still mark the flag so the sync-gate opens; no HTTP call needed.
    @fake.expects(:get_stocks).never

    MarketData.backfill_canonical_stock_external_ids!

    assert AppConfig.get('stock_canonical_backfill_completed_at').present?,
           'flag must be set even with zero legacy rows, otherwise the sync-gate would never open'
  end

  test 'ambiguity guard: flag is still set when some symbols are excluded (forward progress)' do
    Asset.create!(external_id: 'alpaca_uuid-ibit', symbol: 'IBIT', name: 'A', category: 'Stock')
    Asset.create!(external_id: 'alpaca_uuid-aapl', symbol: 'AAPL', name: 'Apple', category: 'Stock')

    @fake.stubs(:get_stocks).returns(stocks_payload([
                                                      # Clean: AAPL.US has one alpaca identifier
                                                      stock_with_alpaca_id(external_id: 'AAPL.US', symbol: 'AAPL'),
                                                      # Ambiguous: LDRC.US has both IBIT and LDRC alpaca identifiers
                                                      { 'external_id' => 'LDRC.US', 'symbol' => 'LDRC', 'name' => 'iShares iBonds', 'type' => 'etf',
                                                        'identifiers' => [
                                                          { 'scheme' => 'alpaca', 'value' => 'us_equity:IBIT' },
                                                          { 'scheme' => 'alpaca', 'value' => 'us_equity:LDRC' }
                                                        ] }
                                                    ]))

    MarketData.backfill_canonical_stock_external_ids!

    assert AppConfig.get('stock_canonical_backfill_completed_at').present?,
           'flag must be set so the rest of the sync can proceed; ambiguous symbols are deferred for manual review'
    assert_equal 'AAPL.US', Asset.find_by(symbol: 'AAPL').external_id,
                 'clean symbols are rewritten as normal'
    assert_equal 'alpaca_uuid-ibit', Asset.find_by(symbol: 'IBIT').external_id,
                 'ambiguous symbol stays as legacy alpaca_<uuid>'
  end
end
