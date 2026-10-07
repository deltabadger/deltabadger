require 'test_helper'

class AlpacaTickerCollisionTest < ActiveSupport::TestCase
  setup do
    @exchange = create(:alpaca_exchange)
    @usd = create(:asset, :usd)
    @coin = create(:asset, :bitcoin)
    @stock = create(:asset, external_id: 'BTC.US', symbol: 'BTC', category: 'Stock')
  end

  def listing(asset)
    { 'base_external_id' => asset.external_id, 'quote_external_id' => @usd.external_id,
      'base' => 'BTC', 'quote' => 'USD', 'ticker' => asset == @coin ? 'BTC/USD' : 'BTC',
      'base_decimals' => 8, 'quote_decimals' => 2, 'price_decimals' => 2,
      'minimum_base_size' => '0.00001', 'minimum_quote_size' => '1' }
  end

  test 'same class display pairs are deduped and reconciled using the current asset category' do
    other = create(:asset, external_id: 'OTHER.US', symbol: 'BTC', category: 'Stock')
    rows = [listing(@stock), listing(other).merge('ticker' => 'OTHER')]
    assert_equal [@stock.id], MarketData.import_tickers!(@exchange, rows)
    assert_equal 1, @exchange.tickers.where(base: 'BTC', quote: 'USD').count
    old = @exchange.tickers.find_by!(base_asset: @stock)
    MarketData.import_tickers!(@exchange, [rows.last], category: 'Stock')
    assert_equal ['OTHER'], @exchange.tickers.where(base: 'BTC', quote: 'USD').pluck(:ticker)
    refute old.reload.available?
    assert_match(/\A__stale_/, old.base)
  end

  test 'both import orders keep both identities and flags' do
    [[@coin, @stock], [@stock, @coin]].each do |order|
      order.each { |asset| MarketData.import_tickers!(@exchange, [listing(asset)]) }
      ids = @exchange.tickers.order(:base_asset_id).pluck(:id)
      order.reverse.each { |asset| MarketData.import_tickers!(@exchange, [listing(asset)]) }
      assert_equal ids, @exchange.tickers.order(:base_asset_id).pluck(:id)
      assert_equal [['BTC', 'BTC', true, true], ['BTC/USD', 'BTC', true, true]],
                   @exchange.tickers.order(:ticker).pluck(:ticker, :base, :available, :trading_enabled)
    end
  end

  test 'one batch retains both asset classes through deduplication' do
    MarketData.import_tickers!(@exchange, [listing(@coin), listing(@stock)])
    assert_equal [@coin.id, @stock.id].sort, @exchange.tickers.pluck(:base_asset_id).sort
  end

  test 'a current listing restores its own tombstone without changing the other class' do
    coin = create(:ticker, exchange: @exchange, base_asset: @coin, quote_asset: @usd,
                           ticker: '__stale_7_BTC/USD', base: '__stale_7_BTC', quote: 'USD', available: false)
    stock = create(:ticker, exchange: @exchange, base_asset: @stock, quote_asset: @usd,
                            ticker: 'BTC', base: 'BTC', quote: 'USD', trading_enabled: false)
    before = stock.attributes
    named = Ticker.asset_ids_named(@exchange.id, 'BTC').sort
    ledger = @exchange.send(:ledger_listings, ['BTC'])
    MarketData.import_tickers!(@exchange, [listing(@coin)])
    assert_equal ['BTC/USD', 'BTC', true], coin.reload.attributes.values_at('ticker', 'base', 'available')
    assert_equal before, stock.reload.attributes
    assert_equal named, Ticker.asset_ids_named(@exchange.id, 'BTC').sort
    assert_equal ledger, @exchange.send(:ledger_listings, ['BTC'])
  end

  test 'direct stock catalogue lookup and sweep leave the crypto row unchanged' do
    MarketData.import_tickers!(@exchange, [listing(@coin), listing(@stock)])
    coin = @exchange.tickers.find_by!(base_asset: @coin)
    before = coin.attributes
    @exchange.send(:sync_existing_exchange_assets_and_tickers!, [{ base: 'BTC', quote: 'USD', ticker: 'BTC', trading_enabled: false }])
    assert_equal before, coin.reload.attributes
    refute @exchange.tickers.find_by!(base_asset: @stock).trading_enabled?
  end

  test 'stock and crypto index members resolve by external identity' do
    MarketData.import_tickers!(@exchange, [listing(@coin), listing(@stock)])
    Ticker.any_instance.stubs(:priced?).returns(true)
    Exchanges::Alpaca.any_instance.stubs(:market_open?).returns(true)
    [@coin, @stock].each do |asset|
      MarketData.stubs(:get_top_coins).returns(Result::Success.new([{ 'id' => asset.external_id, 'market_cap' => 100 }]))
      bot = create(:dca_index, exchange: @exchange, quote_asset: @usd)
      assert_predicate bot.refresh_composition, :success?
      assert_equal [asset.id], bot.composition_tickers.map(&:base_asset_id)
      assert_equal([asset.id], bot.current_index_preview.map { |row| row[:asset_id] })
    end
  end

  test 'pair and basket bots resolve their own asset identity' do
    MarketData.import_tickers!(@exchange, [listing(@coin), listing(@stock)])
    [@coin, @stock].each do |asset|
      pair = create(:dca_single_asset, exchange: @exchange, base_asset: asset, quote_asset: @usd)
      assert_equal asset.id, pair.ticker.base_asset_id
      basket = create(:dca_multi_asset, exchange: @exchange, quote_asset: @usd,
                                        base_assets: [asset], allocations: { asset => 1 })
      assert_equal [asset.id], basket.composition_tickers.map(&:base_asset_id)
      assert_equal asset.id, basket.ticker_for_asset(asset.id).base_asset_id
    end
  end
end

class AlpacaTickerCollisionTest
  test 'each venue sweep touches only its asset class, including wrong-class payloads' do
    aapl = create(:asset, symbol: 'AAPL', external_id: 'AAPL.US', category: 'Stock')
    old_stock = create(:ticker, exchange: @exchange, base_asset: aapl, quote_asset: @usd,
                                base: 'AAPL', quote: 'USD', ticker: 'AAPL')
    eth = create(:asset, :ethereum)
    old_coin = create(:ticker, exchange: @exchange, base_asset: eth, quote_asset: @usd,
                               base: 'ETH', quote: 'USD', ticker: 'ETH/USD')
    MarketData.import_tickers!(@exchange, [listing(@coin), listing(@stock)])
    client = stub
    MarketData.stubs(:client).returns(client)
    MarketData.stubs(:alpaca_listings_degraded?).returns(false)
    MarketData.stubs(:alpaca_crypto_listings_degraded?).returns(false)
    client.stubs(:get_alpaca_listings).returns(Result::Success.new('data' => [listing(@stock), listing(@coin).merge('trading_enabled' => false)]))
    assert_predicate MarketData.sync_alpaca_listings_from_deltabadger!, :success?
    assert_equal '1', AppConfig.get(MarketData::ALPACA_LISTINGS_LAST_GOOD_KEY)
    refute old_stock.reload.available?
    assert old_coin.reload.available?
    assert @exchange.tickers.find_by!(base_asset: @coin).trading_enabled?
    crypto = listing(@coin).merge('base_asset_id' => 'crypto:bitcoin', 'symbol' => 'BTC/USD')
    wrong = listing(@stock).merge('base_asset_id' => 'crypto:BTC.US', 'symbol' => 'BTC/USD', 'trading_enabled' => false)
    client.stubs(:get_alpaca_crypto_listings).returns(Result::Success.new('data' => [crypto, wrong]))
    assert_predicate MarketData.sync_alpaca_crypto_listings_from_deltabadger!, :success?
    refute old_coin.reload.available?
    assert_equal [true, true], @exchange.tickers.find_by!(base_asset: @stock).attributes.values_at('available', 'trading_enabled')
  end
end

class AlpacaTickerCollisionTest
  test 'a cross-class native ticker conflict is refused without stealing a row' do
    MarketData.import_tickers!(@exchange, [listing(@coin)])
    before = @exchange.tickers.map(&:attributes)
    bad = listing(@stock).merge('ticker' => 'BTC/USD')
    assert_raises(ActiveRecord::RecordNotUnique) { MarketData.import_tickers!(@exchange, [bad]) }
    assert_equal before, @exchange.tickers.reload.map(&:attributes)
    assert_raises(ActiveRecord::RecordNotUnique) { MarketData.import_tickers!(@exchange, [listing(@coin), bad]) }
    assert_equal before, @exchange.tickers.reload.map(&:attributes)
  end
end

class AlpacaTickerCollisionTest
  test 'direct stock discovery cannot create a cryptocurrency listing' do
    @exchange.instance_variable_set(:@symbol_to_external_id_hash, { 'BTC' => 'bitcoin', 'USD' => 'usd' })
    info = { base: 'BTC', quote: 'USD', ticker: 'BTC', base_decimals: 8, quote_decimals: 2, price_decimals: 2,
             minimum_base_size: 0.0001, minimum_quote_size: 1 }
    @exchange.send(:sync_existing_exchange_assets_and_tickers!, [info])
    assert_empty @exchange.tickers
  end
end
