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

  # Fix A: the sweep in sync_alpaca_listings must key off EXACTLY what import wrote, not the raw
  # resolved-listing set. So import_tickers! returns the post-dedup base_asset_ids it upserted.
  test 'returns the base_asset_ids it actually upserted (post-dedup keep-set for the sweep)' do
    data = [
      ticker_data(base_ext_id: 'bitcoin', quote_ext_id: 'usd', base: 'BTC', quote: 'USD', ticker: 'BTCUSD'),
      ticker_data(base_ext_id: 'ethereum', quote_ext_id: 'usd', base: 'ETH', quote: 'USD', ticker: 'ETHUSD')
    ]

    written = MarketData.import_tickers!(@exchange, data)

    assert_kind_of Array, written
    assert_equal [@btc.id, @eth.id].sort, written.sort
  end

  test 'returned keep-set excludes a listing dropped by ticker-string dedup' do
    # Two listings collide on the same ticker string; dedup keeps the first (BTC), drops ETH.
    data = [
      ticker_data(base_ext_id: 'bitcoin', quote_ext_id: 'usd', base: 'BTC', quote: 'USD', ticker: 'DUPE'),
      ticker_data(base_ext_id: 'ethereum', quote_ext_id: 'usd', base: 'ETH', quote: 'USD', ticker: 'DUPE')
    ]

    written = MarketData.import_tickers!(@exchange, data)

    assert_equal [@btc.id], written, 'only the base actually upserted is returned, not the deduped-out one'
  end

  test 'returns an empty array (not nil) when nothing is imported' do
    assert_equal [], MarketData.import_tickers!(@exchange, [])
    assert_equal [], MarketData.import_tickers!(@exchange, [
                                                  ticker_data(base_ext_id: 'unknown', quote_ext_id: 'usd', base: 'X', quote: 'USD', ticker: 'XUSD')
                                                ])
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

  # Guards against the nil base_external_id regression that silently dropped these tickers
  test 'imports Hyperliquid tokenized-stock ticker whose base_external_id is an hl:<tokenId> id' do
    # import_tickers! resolution is exchange-agnostic; @exchange (Binance) stands in for any exchange
    aapl = create(:asset, external_id: 'hl:0xAAA', symbol: 'AAPL', name: 'AAPL (Hyperliquid)', category: 'Tokenized Stock')
    usdc = create(:asset, external_id: 'usd-coin', symbol: 'USDC', name: 'USD Coin', category: 'Cryptocurrency')

    data = [ticker_data(base_ext_id: 'hl:0xAAA', quote_ext_id: 'usd-coin', base: 'AAPL', quote: 'USDC', ticker: '@268')]

    MarketData.import_tickers!(@exchange, data)

    t = @exchange.tickers.find_by(ticker: '@268')
    assert_not_nil t, 'ticker must be created for Hyperliquid RWA pair'
    assert_equal aapl.id, t.base_asset_id
    assert_equal usdc.id, t.quote_asset_id
    assert t.available, 'ticker must be available'
    assert t.trading_enabled, 'ticker must be trading_enabled'
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

  # Fix B: transient network errors must PROPAGATE (not be swallowed into Result::Failure) so the
  # job's retry_on can engage. A non-transient failure still returns Result::Failure as before.
  test 'Fix B: re-raises transient network errors so the job can retry' do
    @fake.stubs(:get_stocks).raises(Client::TransientNetworkError, 'Faraday::TimeoutError: Net::ReadTimeout')
    assert_raises(Client::TransientNetworkError) { MarketData.sync_stocks_from_deltabadger! }
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
    # These behavioural tests use tiny (1-2 listing) payloads. The degraded-payload guard's
    # absolute first-run floor would otherwise reject them, so neutralise it here; the guard
    # itself is covered by the dedicated tests below and MarketDataAlpacaListingsDegradedTest.
    MarketData.stubs(:min_healthy_alpaca_listings).returns(0)
    @alpaca = create(:alpaca_exchange)
    @aapl = Asset.create!(external_id: 'AAPL.US', symbol: 'AAPL', name: 'Apple', category: 'Stock')
    @spy = Asset.create!(external_id: 'SPY.US', symbol: 'SPY', name: 'SPDR S&P 500 ETF', category: 'Stock')
    @fake = mock
    MarketData.stubs(:client).returns(@fake)
  end

  def usd_asset
    Asset.find_or_create_by!(external_id: 'usd') do |a|
      a.symbol = 'USD'
      a.name = 'US Dollar'
      a.category = 'Fiat'
    end
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

  # == Fix A: availability fail-safe ==
  # The degraded-payload guard must measure the RESOLVED (importable) universe, not the raw
  # listings.size. A payload that looks big but whose bases no longer resolve locally (the FIGI
  # identity-drift shape) would otherwise pass the raw-count guard, import almost nothing, and let
  # the sweep blank every previously-available ticker — stranding a whole container at AV=0.
  test 'fail-safe: a collapsed RESOLVED count bails out without blanking existing availability' do
    # A previously-available ticker that is NOT present in the next payload.
    msft = Asset.create!(external_id: 'MSFT.US', symbol: 'MSFT', name: 'Microsoft', category: 'Stock')
    usd = usd_asset
    create(:ticker, exchange: @alpaca, base_asset: msft, quote_asset: usd,
                    base: 'MSFT', quote: 'USD', ticker: 'MSFT', available: true)

    # Healthy baseline of 10 importable listings from a prior run.
    AppConfig.set(MarketData::ALPACA_LISTINGS_LAST_GOOD_KEY, '10')

    # Payload LOOKS healthy (10 rows) but only 1 resolves locally — 9 unknown bases.
    rows = [listing_row(base_ext: 'AAPL.US', symbol: 'AAPL')]
    9.times { |i| rows << listing_row(base_ext: "UNKNOWN#{i}.US", symbol: "UNK#{i}") }
    @fake.stubs(:get_alpaca_listings).returns(Result::Success.new('metadata' => { 'count' => rows.size }, 'data' => rows))

    # A degraded payload must be a full no-op: no import (not even the one resolvable row) and no sweep.
    assert_no_difference ['Ticker.count', 'ExchangeAsset.count'] do
      MarketData.sync_alpaca_listings_from_deltabadger!
    end

    assert_nil @alpaca.tickers.find_by(base_asset: @aapl),
               'on a degraded bail we trust none of the payload — not even the rows that resolve'
    assert @alpaca.tickers.find_by(base_asset: msft).available?,
           'a payload whose importable universe collapsed must NOT blank existing availability'
    assert_equal '10', AppConfig.get(MarketData::ALPACA_LISTINGS_LAST_GOOD_KEY),
                 'baseline must not ratchet down on a bailed run'
  end

  test 'fail-safe: a healthy payload still imports and sweeps normally (no false bail)' do
    AppConfig.set(MarketData::ALPACA_LISTINGS_LAST_GOOD_KEY, '2')
    @fake.stubs(:get_alpaca_listings).returns(Result::Success.new(
                                                'metadata' => { 'count' => 2 },
                                                'data' => [listing_row(base_ext: 'AAPL.US', symbol: 'AAPL'),
                                                           listing_row(base_ext: 'SPY.US', symbol: 'SPY')]
                                              ))

    MarketData.sync_alpaca_listings_from_deltabadger!

    assert @alpaca.tickers.find_by(base_asset: @aapl).available?
    assert @alpaca.tickers.find_by(base_asset: @spy).available?
  end

  # Proves the sweep keeps EXACTLY import_tickers!'s post-dedup written set, not the raw resolved-listing
  # set. Two listings resolve but collide on ticker string → dedup writes only the first base (AAPL); the
  # deduped-out base (SPY) must therefore be swept unavailable. The old raw-resolved sweep would keep it.
  test 'fail-safe: sweep keys off the post-dedup written set (deduped-out base is swept unavailable)' do
    usd = usd_asset
    # Pre-existing available ticker for SPY (the base that will be deduped out of the incoming batch).
    create(:ticker, exchange: @alpaca, base_asset: @spy, quote_asset: usd,
                    base: 'SPY', quote: 'USD', ticker: 'SPY', available: true)

    collide = lambda do |base_ext|
      { 'listing_id' => "NASDAQ:#{base_ext}", 'base' => 'SHARED', 'quote' => 'USD', 'ticker' => 'SHARED',
        'base_external_id' => base_ext, 'quote_external_id' => 'USD.FOREX', 'fractionable' => true }
    end
    @fake.stubs(:get_alpaca_listings).returns(Result::Success.new(
                                                'metadata' => { 'count' => 2 },
                                                'data' => [collide.call('AAPL.US'), collide.call('SPY.US')]
                                              ))

    MarketData.sync_alpaca_listings_from_deltabadger!

    assert_not @alpaca.tickers.find_by(base_asset: @spy).available?,
               'a base that import deduped out is NOT in the keep-set and must be swept unavailable'
  end

  # Fix A baseline semantics: ratchet the degraded-guard baseline to the RESOLVED (importable) count,
  # not the raw listings.size. Raw rows = 3, but one base does not resolve locally → resolved = 2.
  test 'fail-safe: baseline ratchets to the resolved count, not raw listings.size' do
    AppConfig.set(MarketData::ALPACA_LISTINGS_LAST_GOOD_KEY, '2')
    @fake.stubs(:get_alpaca_listings).returns(Result::Success.new(
                                                'metadata' => { 'count' => 3 },
                                                'data' => [listing_row(base_ext: 'AAPL.US', symbol: 'AAPL'),
                                                           listing_row(base_ext: 'SPY.US', symbol: 'SPY'),
                                                           listing_row(base_ext: 'NOPE.US', symbol: 'NOPE')]
                                              ))

    MarketData.sync_alpaca_listings_from_deltabadger!

    assert_equal '2', AppConfig.get(MarketData::ALPACA_LISTINGS_LAST_GOOD_KEY),
                 'baseline must reflect importable rows (2), not the 3 raw listings'
  end

  test 'returns Result::Failure when client fails, no DB writes' do
    @fake.stubs(:get_alpaca_listings).returns(Result::Failure.new('boom'))
    assert_no_difference ['Ticker.count', 'Asset.count'] do
      result = MarketData.sync_alpaca_listings_from_deltabadger!
      assert_predicate result, :failure?
    end
  end

  # Fix B: a raised transient error must propagate, not be swallowed into Result::Failure.
  test 'Fix B: re-raises transient network errors so the job can retry' do
    @fake.stubs(:get_alpaca_listings).raises(Client::TransientNetworkError, 'Faraday::TimeoutError: Net::ReadTimeout')
    assert_raises(Client::TransientNetworkError) { MarketData.sync_alpaca_listings_from_deltabadger! }
  end

  # == Listing-import ambiguity guard (post-incident 2026-05-28 Phase 2.5) ==
  # Even with the backfill ambiguity guard preserving legacy alpaca_<uuid> Asset rows,
  # the listing import could still corrupt bots: data-api's ?venue_scheme=alpaca_exchange
  # endpoint returns ONE listing per base_asset_id (lex-smallest listing_id). When
  # EODHD/Alpaca have multiple symbols sharing an ISIN (IBIT and LDRC under
  # US46438F1012), the canonical asset wins lex-smallest = "NASDAQ:IBIT" — so data-api
  # emits a row { base: 'IBIT', base_external_id: 'LDRC.US' }. import_tickers! sees the
  # new IBIT ticker, finds an existing IBIT ticker pointing to the legacy alpaca_<uuid>
  # asset, and tombstones the legacy one to claim the (exchange, ticker) slot. Bot
  # references the legacy ticker by base_asset_id — now tombstoned, unavailable.
  #
  # Guard: any incoming listing whose (exchange, ticker) collides with an existing local
  # ticker pointing at a legacy alpaca_<uuid> Stock asset is DROPPED before import.

  test 'listing-import ambiguity guard: IBIT/LDRC case preserves the legacy ticker' do
    # Setup: legacy alpaca_<uuid> IBIT asset + ticker (the pre-incident state of bot 5).
    legacy_ibit = Asset.create!(external_id: 'alpaca_uuid-ibit', symbol: 'IBIT', name: 'iShares Bitcoin Trust', category: 'Stock')
    create(:exchange_asset, exchange: @alpaca, asset: legacy_ibit, available: true)
    usd = Asset.find_or_create_by!(external_id: 'usd') do |a|
      a.symbol = 'USD'
      a.name = 'US Dollar'
      a.category = 'Fiat'
    end
    create(:exchange_asset, exchange: @alpaca, asset: usd, available: true)
    legacy_ticker = create(:ticker, exchange: @alpaca, base_asset: legacy_ibit, quote_asset: usd,
                                    base: 'IBIT', quote: 'USD', ticker: 'IBIT', available: true)
    # Canonical LDRC.US asset (created by sync_stocks_from_deltabadger! in real flow).
    canonical_ldrc = Asset.create!(external_id: 'LDRC.US', symbol: 'LDRC', name: 'iShares iBonds', category: 'Stock')

    # data-api payload: ONE listing per base_asset_id, lex-smallest listing_id wins. For
    # the LDRC.US asset, "NASDAQ:IBIT" beats "NASDAQ:LDRC" lex — so the payload row's
    # `base` is "IBIT" but `base_external_id` is "LDRC.US" — exactly the IBIT/LDRC shape.
    @fake.stubs(:get_alpaca_listings).returns(Result::Success.new(
                                                'metadata' => { 'count' => 1 },
                                                'data' => [
                                                  listing_row(base_ext: 'LDRC.US', symbol: 'IBIT', listing_id: 'NASDAQ:IBIT')
                                                ]
                                              ))

    MarketData.sync_alpaca_listings_from_deltabadger!

    # The legacy ticker must be UNCHANGED — not tombstoned, still available.
    legacy_ticker.reload
    assert_equal 'IBIT', legacy_ticker.base, 'legacy base must not be tombstoned'
    assert_equal 'IBIT', legacy_ticker.ticker, 'legacy ticker string must not be tombstoned'
    assert legacy_ticker.available?, 'legacy ticker must stay available'
    assert_equal legacy_ibit.id, legacy_ticker.base_asset_id, 'legacy ticker must still point at legacy asset'

    # No NEW ticker for canonical LDRC.US with the same (exchange, ticker) slot.
    canonical_ticker = @alpaca.tickers.where(base_asset_id: canonical_ldrc.id).first
    assert_nil canonical_ticker,
               'incoming listing whose ticker symbol collides with legacy must be dropped, not imported'
  end

  test 'listing-import ambiguity guard: non-colliding listings still import normally' do
    # Standard case: no legacy alpaca_<uuid> ticker with the same symbol. Listing imports normally.
    @fake.stubs(:get_alpaca_listings).returns(Result::Success.new(
                                                'metadata' => { 'count' => 1 },
                                                'data' => [listing_row(base_ext: 'AAPL.US', symbol: 'AAPL')]
                                              ))

    MarketData.sync_alpaca_listings_from_deltabadger!

    t = @alpaca.tickers.find_by(base_asset: @aapl)
    assert t.present?, 'non-colliding listing must be imported'
    assert t.available?
  end

  test 'listing-import ambiguity guard: stale sweep does not touch preserved legacy ticker' do
    # Same setup as the IBIT/LDRC case + an unrelated AAPL listing in the payload.
    legacy_ibit = Asset.create!(external_id: 'alpaca_uuid-ibit', symbol: 'IBIT', name: 'iShares Bitcoin Trust', category: 'Stock')
    create(:exchange_asset, exchange: @alpaca, asset: legacy_ibit, available: true)
    usd = Asset.find_or_create_by!(external_id: 'usd') do |a|
      a.symbol = 'USD'
      a.name = 'US Dollar'
      a.category = 'Fiat'
    end
    create(:exchange_asset, exchange: @alpaca, asset: usd, available: true)
    legacy_ticker = create(:ticker, exchange: @alpaca, base_asset: legacy_ibit, quote_asset: usd,
                                    base: 'IBIT', quote: 'USD', ticker: 'IBIT', available: true)
    Asset.create!(external_id: 'LDRC.US', symbol: 'LDRC', name: 'iShares iBonds', category: 'Stock')

    @fake.stubs(:get_alpaca_listings).returns(Result::Success.new(
                                                'metadata' => { 'count' => 2 },
                                                'data' => [
                                                  listing_row(base_ext: 'LDRC.US', symbol: 'IBIT', listing_id: 'NASDAQ:IBIT'),
                                                  listing_row(base_ext: 'AAPL.US', symbol: 'AAPL')
                                                ]
                                              ))

    MarketData.sync_alpaca_listings_from_deltabadger!

    legacy_ticker.reload
    assert legacy_ticker.available?, 'preserved legacy ticker must not be swept'
    assert_equal 'IBIT', legacy_ticker.base
  end

  # == Degraded-payload guard (incident 2026-06-02) ==
  # The stale-ticker sweep marks unavailable every ticker absent from `incoming`. A PARTIAL
  # data-api response (non-empty but far smaller than the real universe) passed the old
  # `if incoming.any?` guard and blanked the whole exchange — and since the sweep only ever
  # sets available:false (only import sets true) and the job runs once/day, there was no
  # self-heal for ~24h. Guard: bail out of the WHOLE method (import + sweep) when the payload
  # is implausibly small vs the persisted last-good size (or, with no baseline, an absolute floor).

  test 'degraded payload far below last-good leaves availability untouched (no import, no sweep)' do
    # Establish two live tickers via a healthy sync.
    @fake.stubs(:get_alpaca_listings).returns(Result::Success.new(
                                                'metadata' => { 'count' => 2 },
                                                'data' => [listing_row(base_ext: 'AAPL.US', symbol: 'AAPL'),
                                                           listing_row(base_ext: 'SPY.US', symbol: 'SPY')]
                                              ))
    MarketData.sync_alpaca_listings_from_deltabadger!
    assert @alpaca.tickers.find_by(base_asset: @spy).available?

    # Simulate a large steady-state universe, then a degraded run returning only AAPL.
    AppConfig.set(MarketData::ALPACA_LISTINGS_LAST_GOOD_KEY, '100')
    @fake.stubs(:get_alpaca_listings).returns(Result::Success.new(
                                                'metadata' => { 'count' => 1 },
                                                'data' => [listing_row(base_ext: 'AAPL.US', symbol: 'AAPL')]
                                              ))
    MarketData.sync_alpaca_listings_from_deltabadger!

    assert @alpaca.tickers.find_by(base_asset: @spy).available?,
           'a partial payload must NOT sweep the rest of the exchange to unavailable'
    assert_equal '100', AppConfig.get(MarketData::ALPACA_LISTINGS_LAST_GOOD_KEY),
                 'last-good must NOT be overwritten by a degraded run'
  end

  test 'records the last-good incoming count after a healthy sync' do
    @fake.stubs(:get_alpaca_listings).returns(Result::Success.new(
                                                'metadata' => { 'count' => 2 },
                                                'data' => [listing_row(base_ext: 'AAPL.US', symbol: 'AAPL'),
                                                           listing_row(base_ext: 'SPY.US', symbol: 'SPY')]
                                              ))
    MarketData.sync_alpaca_listings_from_deltabadger!

    assert_equal '2', AppConfig.get(MarketData::ALPACA_LISTINGS_LAST_GOOD_KEY),
                 'a healthy sync must persist the incoming size as the new baseline'
  end

  test 'first run with no baseline below the absolute floor skips import entirely' do
    MarketData.stubs(:min_healthy_alpaca_listings).returns(5) # override the setup's 0
    assert_nil AppConfig.get(MarketData::ALPACA_LISTINGS_LAST_GOOD_KEY)
    @fake.stubs(:get_alpaca_listings).returns(Result::Success.new(
                                                'metadata' => { 'count' => 2 },
                                                'data' => [listing_row(base_ext: 'AAPL.US', symbol: 'AAPL'),
                                                           listing_row(base_ext: 'SPY.US', symbol: 'SPY')]
                                              ))

    assert_no_difference 'Ticker.count' do
      MarketData.sync_alpaca_listings_from_deltabadger!
    end
    assert_nil AppConfig.get(MarketData::ALPACA_LISTINGS_LAST_GOOD_KEY),
               'a degraded first run must not record a (bogus) baseline'
  end

  test 'a later healthy sync re-heals availability and updates the baseline' do
    # A ticker left stuck unavailable by a prior blanking incident.
    usd = Asset.find_or_create_by!(external_id: 'usd') do |a|
      a.symbol = 'USD'
      a.name = 'US Dollar'
      a.category = 'Fiat'
    end
    create(:exchange_asset, exchange: @alpaca, asset: usd, available: true)
    stuck = create(:ticker, exchange: @alpaca, base_asset: @spy, quote_asset: usd,
                            base: 'SPY', quote: 'USD', ticker: 'SPY', available: false)

    @fake.stubs(:get_alpaca_listings).returns(Result::Success.new(
                                                'metadata' => { 'count' => 2 },
                                                'data' => [listing_row(base_ext: 'AAPL.US', symbol: 'AAPL'),
                                                           listing_row(base_ext: 'SPY.US', symbol: 'SPY')]
                                              ))
    MarketData.sync_alpaca_listings_from_deltabadger!

    assert stuck.reload.available?, 'a healthy sync must restore availability (import sets available: true)'
    assert_equal '2', AppConfig.get(MarketData::ALPACA_LISTINGS_LAST_GOOD_KEY)
  end

  test 'empty payload does not overwrite an existing last-good baseline' do
    @fake.stubs(:get_alpaca_listings).returns(Result::Success.new(
                                                'metadata' => { 'count' => 1 },
                                                'data' => [listing_row(base_ext: 'AAPL.US', symbol: 'AAPL')]
                                              ))
    MarketData.sync_alpaca_listings_from_deltabadger!
    assert_equal '1', AppConfig.get(MarketData::ALPACA_LISTINGS_LAST_GOOD_KEY)

    @fake.stubs(:get_alpaca_listings).returns(Result::Success.new('metadata' => { 'count' => 0 }, 'data' => []))
    MarketData.sync_alpaca_listings_from_deltabadger!

    assert_equal '1', AppConfig.get(MarketData::ALPACA_LISTINGS_LAST_GOOD_KEY),
                 'an empty payload must not ratchet the baseline down to 0'
  end
end

# Pure predicate for the degraded-payload guard — no DB/fixtures, exact thresholds.
class MarketDataAlpacaListingsDegradedTest < ActiveSupport::TestCase
  test 'no baseline: degraded iff below the absolute floor' do
    MarketData.stubs(:min_healthy_alpaca_listings).returns(1000)
    assert MarketData.alpaca_listings_degraded?(999, nil)
    assert MarketData.alpaca_listings_degraded?(0, nil)
    assert_not MarketData.alpaca_listings_degraded?(1000, nil)
    assert_not MarketData.alpaca_listings_degraded?(6657, nil)
    assert_not MarketData.alpaca_listings_degraded?(6657, '') # blank baseline behaves as "none"
  end

  test 'with baseline: degraded iff below 90% of last-good' do
    assert MarketData.alpaca_listings_degraded?(50, '6657')      # the incident shape
    assert MarketData.alpaca_listings_degraded?(0, '6657')
    assert MarketData.alpaca_listings_degraded?(5990, '6657')    # 5990 < 5991 (=6657*9/10)
    assert_not MarketData.alpaca_listings_degraded?(5991, '6657')
    assert_not MarketData.alpaca_listings_degraded?(6657, '6657') # steady state
    assert_not MarketData.alpaca_listings_degraded?(6656, '6657') # one delisting is fine
  end

  test 'an empty/zero payload is always degraded regardless of baseline (no baseline ratchet to 0)' do
    MarketData.stubs(:min_healthy_alpaca_listings).returns(0)
    assert MarketData.alpaca_listings_degraded?(0, nil)
    assert MarketData.alpaca_listings_degraded?(0, '0')
    assert MarketData.alpaca_listings_degraded?(0, '6657')
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

class MarketDataImportIndicesWeightsTest < ActiveSupport::TestCase
  test 'import_indices! persists the weights map' do
    MarketData.import_indices!([
                                 { 'external_id' => 'nasdaq-100', 'source' => 'deltabadger', 'name' => 'Nasdaq 100',
                                   'top_coins' => %w[AAPL.US MSFT.US], 'weights' => { 'AAPL.US' => 9.12, 'MSFT.US' => 8.41 } }
                               ])

    index = Index.find_by(external_id: 'nasdaq-100', source: 'deltabadger')
    assert_equal({ 'AAPL.US' => 9.12, 'MSFT.US' => 8.41 }, index.weights)
  end

  test 'import_indices! defaults weights to {} when absent (crypto indices)' do
    MarketData.import_indices!([
                                 { 'external_id' => 'layer-1', 'source' => 'coingecko', 'name' => 'Layer 1',
                                   'top_coins' => %w[bitcoin ethereum] }
                               ])

    assert_equal({}, Index.find_by(external_id: 'layer-1').weights)
  end
end

# The one general weight rule (no stock special-case): use Asset.market_cap when known,
# otherwise the index-provided allocation weight, otherwise skip the member.
class MarketDataGetTopCoinsWeightRuleTest < ActiveSupport::TestCase
  setup do
    MarketDataSettings.stubs(:current_provider).returns(MarketDataSettings::PROVIDER_DELTABADGER)
  end

  def nasdaq_index(weights:, top_coins: nil)
    Index.create!(external_id: 'nasdaq-100', source: Index::SOURCE_DELTABADGER, name: 'Nasdaq 100',
                  top_coins: top_coins || weights.keys, weights: weights)
  end

  test 'falls back to the index weight when the asset has no market cap (stocks)' do
    nasdaq_index(weights: { 'AAPL.US' => 9.0, 'MSFT.US' => 8.0 })
    create(:asset, external_id: 'AAPL.US', symbol: 'AAPL', market_cap: nil)
    create(:asset, external_id: 'MSFT.US', symbol: 'MSFT', market_cap: nil)

    result = MarketData.get_top_coins(index_type: 'category', category_id: 'nasdaq-100')
    assert_predicate result, :success?
    by_id = result.data.index_by { |c| c['id'] }
    assert_equal 9.0, by_id['AAPL.US']['market_cap']
    assert_equal 8.0, by_id['MSFT.US']['market_cap']
  end

  test 'prefers a real market cap over the provided weight' do
    nasdaq_index(weights: { 'AAPL.US' => 9.0 })
    create(:asset, external_id: 'AAPL.US', symbol: 'AAPL', market_cap: 500.0)

    result = MarketData.get_top_coins(index_type: 'category', category_id: 'nasdaq-100')
    assert_equal 500.0, result.data.find { |c| c['id'] == 'AAPL.US' }['market_cap']
  end

  test 'skips a member with neither a market cap nor a weight' do
    nasdaq_index(weights: { 'AAPL.US' => 9.0 }, top_coins: %w[AAPL.US NOWEIGHT.US])
    create(:asset, external_id: 'AAPL.US', symbol: 'AAPL', market_cap: nil)
    create(:asset, external_id: 'NOWEIGHT.US', symbol: 'NOW', market_cap: nil)

    result = MarketData.get_top_coins(index_type: 'category', category_id: 'nasdaq-100')
    ids = result.data.map { |c| c['id'] }
    assert_includes ids, 'AAPL.US'
    assert_not_includes ids, 'NOWEIGHT.US'
  end
end
