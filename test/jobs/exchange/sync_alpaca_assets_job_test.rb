require 'test_helper'

class Exchange::SyncAlpacaAssetsJobTest < ActiveSupport::TestCase
  setup do
    @exchange = create(:alpaca_exchange)

    AppConfig.set('alpaca_api_key', 'test_key')
    AppConfig.set('alpaca_api_secret', 'test_secret')
    AppConfig.set('alpaca_mode', 'paper')

    @alpaca_assets_response = [
      { 'id' => 'uuid-aapl', 'symbol' => 'AAPL', 'name' => 'Apple Inc', 'exchange' => 'NASDAQ', 'tradable' => true, 'fractionable' => true },
      { 'id' => 'uuid-msft', 'symbol' => 'MSFT', 'name' => 'Microsoft Corporation',
        'exchange' => 'NASDAQ', 'tradable' => true, 'fractionable' => true },
      { 'id' => 'uuid-goog', 'symbol' => 'GOOG', 'name' => 'Alphabet Inc', 'exchange' => 'NASDAQ', 'tradable' => true, 'fractionable' => false },
      { 'id' => 'uuid-tsla', 'symbol' => 'TSLA', 'name' => 'Tesla Inc', 'exchange' => 'NASDAQ', 'tradable' => false, 'fractionable' => true }
    ]
  end

  test 'creates stock assets from Alpaca API' do
    Clients::Alpaca.any_instance.stubs(:get_assets).returns(Result::Success.new(@alpaca_assets_response))

    assert_difference 'Asset.where(category: "Stock").count', 2 do
      Exchange::SyncAlpacaAssetsJob.perform_now
    end

    aapl = Asset.find_by(external_id: 'alpaca_uuid-aapl')
    assert_equal 'AAPL', aapl.symbol
    assert_equal 'Apple Inc', aapl.name
    assert_equal 'Stock', aapl.category
  end

  test 'creates tickers for each stock paired with USD' do
    Clients::Alpaca.any_instance.stubs(:get_assets).returns(Result::Success.new(@alpaca_assets_response))

    assert_difference 'Ticker.count', 2 do
      Exchange::SyncAlpacaAssetsJob.perform_now
    end

    aapl = Asset.find_by(external_id: 'alpaca_uuid-aapl')
    usd_asset = Asset.find_by(external_id: 'usd')
    ticker = Ticker.find_by(exchange: @exchange, base_asset: aapl, quote_asset: usd_asset)
    assert ticker.present?
    assert_equal 'AAPL', ticker.base
    assert_equal 'USD', ticker.quote
    assert_equal 'AAPL', ticker.ticker
    assert ticker.available?
  end

  test 'creates exchange assets for stocks and USD' do
    Clients::Alpaca.any_instance.stubs(:get_assets).returns(Result::Success.new(@alpaca_assets_response))

    Exchange::SyncAlpacaAssetsJob.perform_now

    aapl = Asset.find_by(external_id: 'alpaca_uuid-aapl')
    usd_asset = Asset.find_by(external_id: 'usd')
    assert ExchangeAsset.exists?(exchange: @exchange, asset: aapl)
    assert ExchangeAsset.exists?(exchange: @exchange, asset: usd_asset)
  end

  test 'skips non-tradable and non-fractionable assets' do
    Clients::Alpaca.any_instance.stubs(:get_assets).returns(Result::Success.new(@alpaca_assets_response))

    Exchange::SyncAlpacaAssetsJob.perform_now

    # GOOG is not fractionable, TSLA is not tradable — both skipped
    assert_nil Asset.find_by(external_id: 'alpaca_uuid-goog')
    assert_nil Asset.find_by(external_id: 'alpaca_uuid-tsla')
  end

  test 'is idempotent — does not duplicate on re-run' do
    Clients::Alpaca.any_instance.stubs(:get_assets).returns(Result::Success.new(@alpaca_assets_response))

    Exchange::SyncAlpacaAssetsJob.perform_now
    assert_no_difference ['Asset.count', 'Ticker.count', 'ExchangeAsset.count'] do
      Exchange::SyncAlpacaAssetsJob.perform_now
    end
  end

  test 'marks stale tickers unavailable' do
    Clients::Alpaca.any_instance.stubs(:get_assets).returns(Result::Success.new(@alpaca_assets_response))
    Exchange::SyncAlpacaAssetsJob.perform_now

    # Second sync with AAPL removed
    reduced_response = @alpaca_assets_response.select { |a| a['symbol'] == 'MSFT' }
    Clients::Alpaca.any_instance.stubs(:get_assets).returns(Result::Success.new(reduced_response))
    Exchange::SyncAlpacaAssetsJob.perform_now

    aapl = Asset.find_by(external_id: 'alpaca_uuid-aapl')
    aapl_ticker = Ticker.find_by(exchange: @exchange, base_asset: aapl)
    refute aapl_ticker.available?

    msft = Asset.find_by(external_id: 'alpaca_uuid-msft')
    msft_ticker = Ticker.find_by(exchange: @exchange, base_asset: msft)
    assert msft_ticker.available?
  end

  test 'handles Alpaca reassigning asset IDs for the same symbol' do
    Clients::Alpaca.any_instance.stubs(:get_assets).returns(Result::Success.new(@alpaca_assets_response))
    Exchange::SyncAlpacaAssetsJob.perform_now

    aapl = Asset.find_by(external_id: 'alpaca_uuid-aapl')
    old_ticker = Ticker.find_by(exchange: @exchange, base_asset: aapl)
    assert old_ticker.available?

    # Alpaca reassigns AAPL to a new UUID
    reassigned_response = [
      { 'id' => 'uuid-aapl-v2', 'symbol' => 'AAPL', 'name' => 'Apple Inc', 'exchange' => 'NASDAQ', 'tradable' => true, 'fractionable' => true },
      @alpaca_assets_response[1] # MSFT unchanged
    ]
    Clients::Alpaca.any_instance.stubs(:get_assets).returns(Result::Success.new(reassigned_response))

    assert_no_difference 'Ticker.count' do
      Exchange::SyncAlpacaAssetsJob.perform_now
    end

    new_asset = Asset.find_by(external_id: 'alpaca_uuid-aapl-v2')
    assert new_asset.present?

    old_ticker.reload
    assert_equal new_asset.id, old_ticker.base_asset_id
    assert old_ticker.available?
  end

  test 'does nothing when Alpaca credentials are not configured' do
    AppConfig.delete('alpaca_api_key')
    AppConfig.delete('alpaca_api_secret')

    assert_no_difference 'Asset.count' do
      Exchange::SyncAlpacaAssetsJob.perform_now
    end
  end

  test 'handles API failure gracefully' do
    Clients::Alpaca.any_instance.stubs(:get_assets).returns(Result::Failure.new('Connection error'))

    assert_no_difference 'Asset.count' do
      Exchange::SyncAlpacaAssetsJob.perform_now
    end
  end

  test 'uses paper mode when configured' do
    AppConfig.set('alpaca_mode', 'paper')

    Clients::Alpaca.any_instance.stubs(:get_assets).returns(Result::Success.new([]))

    assert_nothing_raised do
      Exchange::SyncAlpacaAssetsJob.perform_now
    end
  end

  test 'creates the Alpaca USD asset with the canonical fiat color' do
    Clients::Alpaca.any_instance.stubs(:get_assets).returns(Result::Success.new(@alpaca_assets_response))

    assert_nil Asset.find_by(external_id: 'usd')
    Exchange::SyncAlpacaAssetsJob.perform_now

    usd = Asset.find_by(external_id: 'usd')
    assert_equal '#355E3B', usd.color
    assert_equal 'Fiat', usd.category
  end

  test 'backfills color on an existing uncolored usd row' do
    Asset.create!(external_id: 'usd', symbol: 'USD', name: 'US Dollar', category: 'Fiat')
    Clients::Alpaca.any_instance.stubs(:get_assets).returns(Result::Success.new(@alpaca_assets_response))

    Exchange::SyncAlpacaAssetsJob.perform_now

    assert_equal '#355E3B', Asset.find_by(external_id: 'usd').color
  end

  test 'links the colored usd asset to the Alpaca exchange so the balance path resolves it' do
    Clients::Alpaca.any_instance.stubs(:get_assets).returns(Result::Success.new(@alpaca_assets_response))

    Exchange::SyncAlpacaAssetsJob.perform_now

    usd = Asset.find_by(external_id: 'usd')
    assert ExchangeAsset.exists?(exchange: @exchange, asset: usd)
    # asset_from_symbol('USD') is how get_balances resolves cash (alpaca.rb:128)
    assert_equal usd, @exchange.send(:asset_from_symbol, 'USD')
  end

  test 'applies stock colors from the data-api colors map (hosted)' do
    Clients::Alpaca.any_instance.stubs(:get_assets).returns(Result::Success.new(@alpaca_assets_response))
    MarketData.stubs(:stock_colors).returns('AAPL' => '#FF0000')

    Exchange::SyncAlpacaAssetsJob.perform_now

    assert_equal '#FF0000', Asset.find_by(external_id: 'alpaca_uuid-aapl').color
    # MSFT absent from the map → no color
    assert_nil Asset.find_by(external_id: 'alpaca_uuid-msft').color
  end

  test 'updates name and color on existing alpaca assets (find_or_create gap fix)' do
    existing = Asset.create!(external_id: 'alpaca_uuid-aapl', symbol: 'AAPL', name: 'Stale Name', category: 'Stock')
    assert_nil existing.color

    Clients::Alpaca.any_instance.stubs(:get_assets).returns(Result::Success.new(@alpaca_assets_response))
    MarketData.stubs(:stock_colors).returns('AAPL' => '#FF0000')

    Exchange::SyncAlpacaAssetsJob.perform_now

    existing.reload
    assert_equal 'Apple Inc', existing.name
    assert_equal '#FF0000', existing.color
  end

  test 'does not clear an existing color when the map lacks the symbol (non-destructive)' do
    Asset.create!(external_id: 'alpaca_uuid-msft', symbol: 'MSFT', name: 'Microsoft Corporation',
                  category: 'Stock', color: '#123456')

    Clients::Alpaca.any_instance.stubs(:get_assets).returns(Result::Success.new(@alpaca_assets_response))
    MarketData.stubs(:stock_colors).returns('AAPL' => '#FF0000') # no MSFT entry

    Exchange::SyncAlpacaAssetsJob.perform_now

    assert_equal '#123456', Asset.find_by(external_id: 'alpaca_uuid-msft').color
  end

  test 'completes the sync when the colors map is empty (best-effort, free mode)' do
    Clients::Alpaca.any_instance.stubs(:get_assets).returns(Result::Success.new(@alpaca_assets_response))
    MarketData.stubs(:stock_colors).returns({})

    assert_difference 'Asset.where(category: "Stock").count', 2 do
      assert_nothing_raised { Exchange::SyncAlpacaAssetsJob.perform_now }
    end
    assert_nil Asset.find_by(external_id: 'alpaca_uuid-aapl').color
  end

  # --- Hosted mode (R4-F4 / F4) -------------------------------------------------------------
  # On hosted, stock assets + tickers come from data-api. SyncAlpacaAssetsJob would otherwise
  # create duplicate alpaca_<uuid> rows alongside canonical ones, so it must early-return.

  test 'hosted mode: early-returns without touching Alpaca client or DB' do
    MarketDataSettings.stubs(:deltabadger?).returns(true)

    Clients::Alpaca.any_instance.expects(:get_assets).never
    assert_no_difference ['Asset.count', 'Ticker.count', 'ExchangeAsset.count'] do
      Exchange::SyncAlpacaAssetsJob.perform_now
    end
  end

  test 'hosted mode: does not require Alpaca credentials (all responsibilities moved to data-api)' do
    MarketDataSettings.stubs(:deltabadger?).returns(true)
    AppConfig.delete('alpaca_api_key')
    AppConfig.delete('alpaca_api_secret')

    assert_nothing_raised { Exchange::SyncAlpacaAssetsJob.perform_now }
  end

  # --- Free-mode regression (R4-F4 / F4) ----------------------------------------------------
  # Open-source containers must keep today's per-user Alpaca-driven path byte-for-byte.

  test 'free mode regression: stocks/tickers/exchange_assets/USD still created as today' do
    MarketDataSettings.stubs(:deltabadger?).returns(false)
    Clients::Alpaca.any_instance.stubs(:get_assets).returns(Result::Success.new(@alpaca_assets_response))

    assert_difference 'Asset.where(category: "Stock").count', 2 do
      Exchange::SyncAlpacaAssetsJob.perform_now
    end
    assert Asset.find_by(external_id: 'usd').present?, "free-mode must still create 'usd' (wizard invariant)"
    assert Ticker.find_by(exchange: @exchange, base_asset: Asset.find_by(external_id: 'alpaca_uuid-aapl')).present?
  end

  # --- Crypto assets (Alpaca crypto pairs mapped to canonical CoinGecko assets) -----------

  test 'creates crypto tickers mapped to the canonical CoinGecko asset' do
    Clients::Alpaca.any_instance.stubs(:get_assets)
                   .with(status: 'active', asset_class: 'us_equity')
                   .returns(Result::Success.new([]))
    Clients::Alpaca.any_instance.stubs(:get_assets)
                   .with(status: 'active', asset_class: 'crypto')
                   .returns(Result::Success.new([
                                                  { 'symbol' => 'AAVE/USD', 'tradable' => true, 'min_order_size' => '0.01',
                                                    'min_trade_increment' => '0.001', 'price_increment' => '0.01' }
                                                ]))

    assert_difference 'Ticker.count', 1 do
      Exchange::SyncAlpacaAssetsJob.perform_now
    end

    aave = Asset.find_by(external_id: 'aave')
    assert aave.present?
    assert_equal 'Cryptocurrency', aave.category

    ticker = Ticker.find_by(exchange: @exchange, base_asset: aave)
    assert_equal 'AAVE/USD', ticker.ticker
    assert_equal 'AAVE', ticker.base
    assert_equal 'USD', ticker.quote
    assert_equal 3, ticker.base_decimals
    assert_equal 2, ticker.price_decimals
  end

  test 'skips a non-USD-quoted crypto pair' do
    Clients::Alpaca.any_instance.stubs(:get_assets)
                   .with(status: 'active', asset_class: 'us_equity')
                   .returns(Result::Success.new([]))
    Clients::Alpaca.any_instance.stubs(:get_assets)
                   .with(status: 'active', asset_class: 'crypto')
                   .returns(Result::Success.new([{ 'symbol' => 'AAVE/USDT', 'tradable' => true }]))

    assert_no_difference ['Asset.where(category: "Cryptocurrency").count', 'Ticker.count'] do
      Exchange::SyncAlpacaAssetsJob.perform_now
    end
  end

  test 'reuses an existing canonical crypto asset instead of creating a duplicate' do
    aave = create(:asset, external_id: 'aave', symbol: 'AAVE', category: 'Cryptocurrency')

    Clients::Alpaca.any_instance.stubs(:get_assets)
                   .with(status: 'active', asset_class: 'us_equity')
                   .returns(Result::Success.new([]))
    Clients::Alpaca.any_instance.stubs(:get_assets)
                   .with(status: 'active', asset_class: 'crypto')
                   .returns(Result::Success.new([{ 'symbol' => 'AAVE/USD', 'tradable' => true }]))

    assert_no_difference 'Asset.where(category: "Cryptocurrency").count' do
      Exchange::SyncAlpacaAssetsJob.perform_now
    end

    ticker = Ticker.find_by(exchange: @exchange, base_asset: aave)
    assert ticker.present?
  end

  test 'skips crypto symbols not present in the CoinGecko id map' do
    Clients::Alpaca.any_instance.stubs(:get_assets)
                   .with(status: 'active', asset_class: 'us_equity')
                   .returns(Result::Success.new([]))
    Clients::Alpaca.any_instance.stubs(:get_assets)
                   .with(status: 'active', asset_class: 'crypto')
                   .returns(Result::Success.new([{ 'symbol' => 'UNKNOWNCOIN/USD', 'tradable' => true }]))

    assert_no_difference ['Asset.where(category: "Cryptocurrency").count', 'Ticker.count'] do
      Exchange::SyncAlpacaAssetsJob.perform_now
    end
  end

  test 'skips non-tradable crypto assets' do
    Clients::Alpaca.any_instance.stubs(:get_assets)
                   .with(status: 'active', asset_class: 'us_equity')
                   .returns(Result::Success.new([]))
    Clients::Alpaca.any_instance.stubs(:get_assets)
                   .with(status: 'active', asset_class: 'crypto')
                   .returns(Result::Success.new([{ 'symbol' => 'AAVE/USD', 'tradable' => false }]))

    assert_no_difference ['Asset.where(category: "Cryptocurrency").count', 'Ticker.count'] do
      Exchange::SyncAlpacaAssetsJob.perform_now
    end
  end

  test 'marks a delisted crypto ticker unavailable without touching others' do
    Clients::Alpaca.any_instance.stubs(:get_assets)
                   .with(status: 'active', asset_class: 'us_equity')
                   .returns(Result::Success.new([]))
    Clients::Alpaca.any_instance.stubs(:get_assets)
                   .with(status: 'active', asset_class: 'crypto')
                   .returns(Result::Success.new([
                                                  { 'symbol' => 'AAVE/USD', 'tradable' => true },
                                                  { 'symbol' => 'BTC/USD', 'tradable' => true }
                                                ]))
    Exchange::SyncAlpacaAssetsJob.perform_now

    # Second sync with AAVE delisted (BTC stays, so synced ids are never empty — the
    # zero-synced-ids guard exists to protect against a transient API glitch wiping
    # everything, not to block a genuine single-asset delisting).
    Clients::Alpaca.any_instance.stubs(:get_assets)
                   .with(status: 'active', asset_class: 'crypto')
                   .returns(Result::Success.new([{ 'symbol' => 'BTC/USD', 'tradable' => true }]))
    Exchange::SyncAlpacaAssetsJob.perform_now

    aave_ticker = Ticker.find_by(exchange: @exchange, base_asset: Asset.find_by(external_id: 'aave'))
    refute aave_ticker.available?

    btc_ticker = Ticker.find_by(exchange: @exchange, base_asset: Asset.find_by(external_id: 'bitcoin'))
    assert btc_ticker.available?
  end

  test 'does not touch existing crypto tickers when the crypto API call fails' do
    Clients::Alpaca.any_instance.stubs(:get_assets)
                   .with(status: 'active', asset_class: 'us_equity')
                   .returns(Result::Success.new([]))
    Clients::Alpaca.any_instance.stubs(:get_assets)
                   .with(status: 'active', asset_class: 'crypto')
                   .returns(Result::Success.new([{ 'symbol' => 'AAVE/USD', 'tradable' => true }]))
    Exchange::SyncAlpacaAssetsJob.perform_now

    Clients::Alpaca.any_instance.stubs(:get_assets)
                   .with(status: 'active', asset_class: 'crypto')
                   .returns(Result::Failure.new('connection error'))
    Exchange::SyncAlpacaAssetsJob.perform_now

    aave_ticker = Ticker.find_by(exchange: @exchange, base_asset: Asset.find_by(external_id: 'aave'))
    assert aave_ticker.available?, 'a transient crypto-endpoint failure must not mark existing crypto tickers stale'
  end

  test 'enqueues CoinGecko backfill for a newly created crypto asset' do
    Clients::Alpaca.any_instance.stubs(:get_assets)
                   .with(status: 'active', asset_class: 'us_equity')
                   .returns(Result::Success.new([]))
    Clients::Alpaca.any_instance.stubs(:get_assets)
                   .with(status: 'active', asset_class: 'crypto')
                   .returns(Result::Success.new([{ 'symbol' => 'AAVE/USD', 'tradable' => true }]))

    Asset::FetchDataFromCoingeckoJob.expects(:perform_later).at_least_once

    Exchange::SyncAlpacaAssetsJob.perform_now
  end
end
