require 'test_helper'

class ExchangeMarketTest < ActiveSupport::TestCase
  setup do
    @honeymaker_exchange = mock('honeymaker_exchange')
    Honeymaker.stubs(:exchange).with('binance').returns(@honeymaker_exchange)

    @exchange = create(:binance_exchange)
    @market = ExchangeMarket.new(@exchange)
  end

  test 'symbol finds ticker by base and quote' do
    tickers = [
      { ticker: 'BTCUSDT', base: 'BTC', quote: 'USDT' },
      { ticker: 'ETHUSDT', base: 'ETH', quote: 'USDT' }
    ]
    @honeymaker_exchange.stubs(:tickers_info).returns(Honeymaker::Result::Success.new(tickers))

    assert_equal 'ETHUSDT', @market.symbol('ETH', 'USDT')
  end

  test 'symbol returns nil when pair not found' do
    @honeymaker_exchange.stubs(:tickers_info).returns(Honeymaker::Result::Success.new([]))

    assert_nil @market.symbol('NOPE', 'USDT')
  end

  test 'current_price delegates to honeymaker get_price' do
    @honeymaker_exchange.stubs(:get_price).with('BTCUSDT').returns(
      Honeymaker::Result::Success.new(BigDecimal('67124.56'))
    )

    result = @market.current_price('BTCUSDT')

    assert result.success?
    assert_equal BigDecimal('67124.56'), result.data
  end

  test 'current_price wraps failure' do
    @honeymaker_exchange.stubs(:get_price).with('BTCUSDT').returns(
      Honeymaker::Result::Failure.new('timeout')
    )

    result = @market.current_price('BTCUSDT')

    assert result.failure?
  end

  test 'current_bid_price returns bid from bid_ask' do
    @honeymaker_exchange.stubs(:get_bid_ask).with('BTCUSDT').returns(
      Honeymaker::Result::Success.new({ bid: BigDecimal('67123'), ask: BigDecimal('67125') })
    )

    result = @market.current_bid_price('BTCUSDT')

    assert result.success?
    assert_equal BigDecimal('67123'), result.data
  end

  test 'current_ask_price returns ask from bid_ask' do
    @honeymaker_exchange.stubs(:get_bid_ask).with('BTCUSDT').returns(
      Honeymaker::Result::Success.new({ bid: BigDecimal('67123'), ask: BigDecimal('67125') })
    )

    result = @market.current_ask_price('BTCUSDT')

    assert result.success?
    assert_equal BigDecimal('67125'), result.data
  end

  test 'base_decimals from find_ticker' do
    @honeymaker_exchange.stubs(:find_ticker).with('BTCUSDT').returns(
      Honeymaker::Result::Success.new({ ticker: 'BTCUSDT', base_decimals: 8 })
    )

    result = @market.base_decimals('BTCUSDT')

    assert result.success?
    assert_equal 8, result.data
  end

  test 'quote_decimals from find_ticker' do
    @honeymaker_exchange.stubs(:find_ticker).with('BTCUSDT').returns(
      Honeymaker::Result::Success.new({ ticker: 'BTCUSDT', quote_decimals: 2 })
    )

    result = @market.quote_decimals('BTCUSDT')

    assert result.success?
    assert_equal 2, result.data
  end

  test 'minimum_order_parameters from find_ticker' do
    @honeymaker_exchange.stubs(:find_ticker).with('BTCUSDT').returns(
      Honeymaker::Result::Success.new({
                                        ticker: 'BTCUSDT',
                                        minimum_base_size: '0.00001',
                                        minimum_quote_size: '5.00'
                                      })
    )

    result = @market.minimum_order_parameters('BTCUSDT')

    assert result.success?
    assert_equal BigDecimal('5.0'), result.data[:minimum]
    assert_equal BigDecimal('5.0'), result.data[:minimum_quote]
    assert_equal BigDecimal('0.00001'), result.data[:minimum_limit]
    assert_equal 'quote', result.data[:side]
  end

  test 'all_symbols returns base/quote hashes' do
    symbols = [{ base: 'BTC', quote: 'USDT' }, { base: 'ETH', quote: 'USDT' }]
    @honeymaker_exchange.stubs(:symbols).returns(Honeymaker::Result::Success.new(symbols))

    result = @market.all_symbols('test_cache_key')

    assert result.success?
    assert_equal 2, result.data.size
    assert_equal 'BTC', result.data.first[:base]
  end

  test 'current_fee returns default' do
    assert_equal 0.1, @market.current_fee
  end

  test 'subaccounts returns empty array' do
    result = @market.subaccounts(nil)

    assert result.success?
    assert_equal [], result.data
  end

  test 'for class method creates market from exchange_id' do
    original = Rails.configuration.dry_run
    Rails.configuration.dry_run = false
    market = ExchangeMarket.for(@exchange.id)

    assert_instance_of ExchangeMarket, market
  ensure
    Rails.configuration.dry_run = original
  end

  test 'for returns fake market in dry_run mode' do
    original = Rails.configuration.dry_run
    Rails.configuration.dry_run = true
    market = ExchangeMarket.for(@exchange.id)

    assert_instance_of ExchangeMarket::Fake, market
  ensure
    Rails.configuration.dry_run = original
  end
end
