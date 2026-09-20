require 'test_helper'

# Covers the available (listed) vs trading_enabled (native status) contract in the
# self-hosted/CoinGecko sync path. Returned pairs are always re-listed; native status
# rides on trading_enabled; pairs absent from the feed are swept to available: false.
class Exchange::SynchronizerTest < ActiveSupport::TestCase
  setup do
    @exchange = create(:binance_exchange)
    @btc = create(:asset, :bitcoin)
    @usd = create(:asset, :usd)
    @eth = create(:asset, :ethereum)
  end

  def sync(tickers_info)
    @exchange.send(:sync_existing_exchange_assets_and_tickers!, tickers_info)
  end

  test 'a returned halted pair stays listed but is marked not trading_enabled' do
    ticker = create(:ticker, exchange: @exchange, base_asset: @btc, quote_asset: @usd,
                             base: 'BTC', quote: 'USD', ticker: 'BTCUSD')

    sync([{ base: 'BTC', quote: 'USD', available: true, trading_enabled: false }])

    ticker.reload
    assert ticker.available, 'returned pair stays listed'
    assert_not ticker.trading_enabled, 'native halted status rides on trading_enabled'
  end

  test 'a previously delisted pair is re-listed when it returns in the feed' do
    ticker = create(:ticker, exchange: @exchange, base_asset: @btc, quote_asset: @usd,
                             base: 'BTC', quote: 'USD', ticker: 'BTCUSD', available: false)

    sync([{ base: 'BTC', quote: 'USD', available: true, trading_enabled: true }])

    assert ticker.reload.available, 'returned pair is re-listed'
    assert ticker.trading_enabled
  end

  test 'a pair absent from the feed is swept to available: false' do
    btc_usd = create(:ticker, exchange: @exchange, base_asset: @btc, quote_asset: @usd,
                              base: 'BTC', quote: 'USD', ticker: 'BTCUSD')
    eth_usd = create(:ticker, exchange: @exchange, base_asset: @eth, quote_asset: @usd,
                              base: 'ETH', quote: 'USD', ticker: 'ETHUSD')

    # Only BTC/USD is returned this sync.
    sync([{ base: 'BTC', quote: 'USD', available: true, trading_enabled: true }])

    assert btc_usd.reload.available
    assert_not eth_usd.reload.available, 'pair absent from the feed is delisted'
  end
  # B10. The self-hosted twin of data-api's join bug: the CoinGecko symbol->coin_id map is keyed on
  # CoinGecko's casing while the venue reports its own. Bitget publishes baseCoin "rON" where
  # CoinGecko says "RON", so external_id_from_symbol missed and `next if blank?` dropped a live
  # pair with no trace. Fixing only data-api leaves every CoinGecko-provider install blind.
  test 'resolves a CoinGecko symbol to a venue symbol case-insensitively' do
    ron = create(:asset, symbol: 'RON', name: 'Ronin', external_id: 'ronin')
    @exchange.send(:set_symbol_to_external_id_hash,
                   [{ 'base' => 'RON', 'coin_id' => 'ronin', 'target' => 'USD', 'target_coin_id' => 'usd' }])

    sync([{ base: 'rON', quote: 'USD', ticker: 'rONUSD', available: true, trading_enabled: true,
            minimum_base_size: 0.1, minimum_quote_size: 5, maximum_base_size: nil, maximum_quote_size: nil,
            base_decimals: 4, quote_decimals: 2, price_decimals: 2 }])

    ticker = @exchange.tickers.find_by(base: 'rON', quote: 'USD')
    assert_not_nil ticker, 'a mixed-case venue symbol must still resolve'
    assert_equal ron.id, ticker.base_asset_id
  end

  # --- CoinGecko is only crawled for pairs we cannot already resolve -------------------------
  #
  # The crawl's ONLY product is @symbol_to_external_id_hash, and that map is read only in the
  # new-ticker branch. So a sync whose venue payload is entirely pairs we already hold needs no
  # market-data call at all. WebMock's disable_net_connect! backs the `.never` expectation up:
  # an unstubbed request would fail the test outright.

  def sync_all(exchange, tickers_info, **kwargs)
    MarketData.stubs(:configured?).returns(true)
    exchange.stubs(:get_tickers_info).returns(Result::Success.new(tickers_info))
    exchange.sync_tickers_and_assets_with_external_data(**kwargs)
  end

  def info(base:, quote:)
    { base:, quote:, ticker: "#{base}#{quote}", available: true, trading_enabled: true,
      minimum_base_size: 0.1, minimum_quote_size: 5, maximum_base_size: nil, maximum_quote_size: nil,
      base_decimals: 4, quote_decimals: 2, price_decimals: 2 }
  end

  test 'does not reach CoinGecko when every reported pair is already a ticker' do
    create(:ticker, exchange: @exchange, base_asset: @btc, quote_asset: @usd,
                    base: 'BTC', quote: 'USD', ticker: 'BTCUSD')
    MarketData.coingecko.expects(:get_exchange_tickers_by_id).never

    assert sync_all(@exchange, [info(base: 'BTC', quote: 'USD')]).success?
  end

  test 'crawls when the venue reports a pair we have never seen' do
    create(:ticker, exchange: @exchange, base_asset: @btc, quote_asset: @usd,
                    base: 'BTC', quote: 'USD', ticker: 'BTCUSD')
    MarketData.coingecko.expects(:get_exchange_tickers_by_id).once.returns(
      Result::Success.new([{ 'base' => 'ETH', 'coin_id' => 'ethereum',
                             'target' => 'USD', 'target_coin_id' => 'usd' }])
    )

    sync_all(@exchange, [info(base: 'BTC', quote: 'USD'), info(base: 'ETH', quote: 'USD')])

    assert @exchange.tickers.exists?(base: 'ETH', quote: 'USD'), 'the new pair is created'
  end

  # The trap that sinks a locally-built symbol map: a fiat Asset row is ONLY ever minted by
  # create_missing_assets!, fed by eodhd_external_id_for_symbol inside the crawl. Deciding on
  # (base, quote) pairs rather than on resolvable symbols is what keeps this working.
  test "mints the fiat asset for a venue's first pair in a new fiat quote" do
    assert_nil Asset.find_by(symbol: 'EUR')
    MarketData.coingecko.expects(:get_exchange_tickers_by_id).once.returns(
      Result::Success.new([{ 'base' => 'BTC', 'coin_id' => 'bitcoin',
                             'target' => 'EUR', 'target_coin_id' => nil }])
    )

    sync_all(@exchange, [info(base: 'BTC', quote: 'EUR')])

    eur = Asset.find_by(symbol: 'EUR')
    assert_not_nil eur, 'the quote fiat asset is created by the crawl'
    assert @exchange.tickers.exists?(base: 'BTC', quote: 'EUR')
  end

  # Rails.cache is :null_store in test, so a cache-backed throttle would assert vacuously —
  # every write would "succeed" and the throttle would never be exercised. Swap in a real store.
  def with_memory_cache
    original = Rails.cache
    Rails.cache = ActiveSupport::Cache::MemoryStore.new
    yield
  ensure
    Rails.cache = original
  end

  test 'crawls at most once a day per venue' do
    crawl = Result::Success.new([{ 'base' => 'ETH', 'coin_id' => 'ethereum',
                                   'target' => 'USD', 'target_coin_id' => 'usd' }])
    MarketData.coingecko.expects(:get_exchange_tickers_by_id).once.returns(crawl)

    with_memory_cache do
      2.times { sync_all(@exchange, [info(base: 'ETH', quote: 'USD'), info(base: 'XRP', quote: 'USD')]) }
    end
  end

  # force bypasses the WINDOW, not the pair check — setup and `rake seed:generate` drive every venue
  # in one process, and on a fresh database every pair is new anyway.
  test 'force bypasses the daily crawl window' do
    crawl = Result::Success.new([{ 'base' => 'ETH', 'coin_id' => 'ethereum',
                                   'target' => 'USD', 'target_coin_id' => 'usd' }])
    MarketData.coingecko.expects(:get_exchange_tickers_by_id).twice.returns(crawl)

    with_memory_cache do
      2.times do
        sync_all(@exchange, [info(base: 'ETH', quote: 'USD'), info(base: 'XRP', quote: 'USD')], force: true)
      end
    end
  end

  # Discovery is best-effort. Before this change the crawl ran FIRST and returned its failure, so a
  # single CoinGecko hiccup left trading_enabled, min/max sizes and the decimals stale fleet-wide —
  # and those size orders.
  test 'a crawl that returns a failure still updates the venue catalogue' do
    ticker = create(:ticker, exchange: @exchange, base_asset: @btc, quote_asset: @usd,
                             base: 'BTC', quote: 'USD', ticker: 'BTCUSD', trading_enabled: true)
    MarketData.coingecko.stubs(:get_exchange_tickers_by_id).returns(Result::Failure.new('429 rate limited'))

    result = sync_all(@exchange, [info(base: 'BTC', quote: 'USD').merge(trading_enabled: false),
                                  info(base: 'ETH', quote: 'USD')])

    assert result.success?
    assert_not ticker.reload.trading_enabled, 'the venue catalogue is applied anyway'
  end

  test 'a crawl that raises still updates the venue catalogue' do
    ticker = create(:ticker, exchange: @exchange, base_asset: @btc, quote_asset: @usd,
                             base: 'BTC', quote: 'USD', ticker: 'BTCUSD', trading_enabled: true)
    MarketData.coingecko.stubs(:get_exchange_tickers_by_id).raises(Client::TransientNetworkError.new('boom'))

    result = sync_all(@exchange, [info(base: 'BTC', quote: 'USD').merge(trading_enabled: false),
                                  info(base: 'ETH', quote: 'USD')])

    assert result.success?
    assert_not ticker.reload.trading_enabled, 'a raise is not a reason to skip the catalogue'
  end

  # Solid Queue signals its semaphore when a job FINISHES; `duration:` only reclaims a lock from a
  # crashed worker. So the bulk backfill needs a real daily claim, not the concurrency control.
  test 'the new-asset backfill is enqueued at most once a day' do
    # Each venue must mint an asset the other did not, or the second sync returns on
    # `new_crypto_assets.empty?` and the daily claim is never reached.
    xrp_crawl = Result::Success.new([{ 'base' => 'XRP', 'coin_id' => 'ripple',
                                       'target' => 'USD', 'target_coin_id' => 'usd' }])
    sol_crawl = Result::Success.new([{ 'base' => 'SOL', 'coin_id' => 'solana',
                                       'target' => 'USD', 'target_coin_id' => 'usd' }])
    other = create(:kraken_exchange)
    MarketData.coingecko.stubs(:get_exchange_tickers_by_id).returns(xrp_crawl, sol_crawl)
    Asset::FetchAllAssetsDataFromCoingeckoJob.expects(:perform_later).once

    with_memory_cache do
      sync_all(@exchange, [info(base: 'XRP', quote: 'USD')])
      sync_all(other, [info(base: 'SOL', quote: 'USD')])
    end
  end
end
