require 'test_helper'

class Exchanges::HyperliquidTest < ActiveSupport::TestCase
  VALID_WALLET = '0x1234567890abcdef1234567890abcdef12345678'.freeze
  VALID_AGENT_KEY = "0x#{'ab' * 32}".freeze

  setup do
    @exchange = create(:hyperliquid_exchange)
  end

  test 'coingecko_id returns hyperliquid-spot' do
    assert_equal 'hyperliquid-spot', @exchange.coingecko_id
  end

  test 'known_errors returns expected error messages' do
    errors = @exchange.known_errors
    assert errors[:insufficient_funds].present?
    assert errors[:invalid_key].present?
  end

  test 'minimum_amount_logic returns base' do
    assert_equal :base, @exchange.minimum_amount_logic(side: :buy, order_type: :limit_order)
    assert_equal :base, @exchange.minimum_amount_logic(side: :sell, order_type: :limit_order)
  end

  test 'set_client creates a Honeymaker Hyperliquid client' do
    @exchange.set_client
    assert_kind_of Honeymaker::Clients::Hyperliquid, @exchange.send(:client)
  end

  test 'set_client with api_key stores the api_key' do
    api_key = create(:api_key, exchange: @exchange,
                               raw_key: VALID_WALLET,
                               raw_secret: VALID_AGENT_KEY)
    @exchange.set_client(api_key: api_key)
    assert_equal api_key, @exchange.api_key
  end

  test 'requires_passphrase? returns false' do
    assert_equal false, @exchange.requires_passphrase?
  end

  test 'get_tickers_info returns formatted ticker data' do
    @exchange.set_client
    client = @exchange.send(:client)

    spot_meta = {
      'tokens' => [
        { 'name' => 'PURR', 'index' => 1, 'szDecimals' => 0 },
        { 'name' => 'USDC', 'index' => 0, 'szDecimals' => 2 },
        { 'name' => 'HYPE', 'index' => 2, 'szDecimals' => 2 }
      ],
      'universe' => [
        { 'name' => 'PURR/USDC', 'tokens' => [1, 0], 'index' => 1000 },
        { 'name' => 'HYPE/USDC', 'tokens' => [2, 0], 'index' => 1001 }
      ]
    }
    client.stubs(:spot_meta).returns(Result::Success.new(spot_meta))

    result = @exchange.get_tickers_info(force: true)
    assert result.success?
    assert_equal 2, result.data.size

    purr_ticker = result.data.find { |t| t[:ticker] == 'PURR/USDC' }
    assert_equal 'PURR', purr_ticker[:base]
    assert_equal 'USDC', purr_ticker[:quote]
    assert_equal 0, purr_ticker[:base_decimals]
    assert purr_ticker[:available]
  end

  test 'get_tickers_prices returns price hash' do
    @exchange.set_client
    client = @exchange.send(:client)

    mids_data = { 'PURR/USDC' => '2.50', 'HYPE/USDC' => '25.00' }
    client.stubs(:all_mids).returns(Result::Success.new(mids_data))

    usdc = create(:asset, external_id: 'usdc', symbol: 'USDC', name: 'USDC')
    purr = create(:asset, external_id: 'purr', symbol: 'PURR', name: 'Purr')
    create(:ticker, exchange: @exchange, base_asset: purr, quote_asset: usdc,
                    ticker: 'PURR/USDC', base: 'PURR', quote: 'USDC')

    result = @exchange.get_tickers_prices(force: true)
    assert result.success?
    assert_equal '2.50'.to_d, result.data['PURR/USDC']
  end

  test 'get_last_price returns mid price' do
    @exchange.set_client
    client = @exchange.send(:client)

    usdc = create(:asset, external_id: 'usdc', symbol: 'USDC', name: 'USDC')
    purr = create(:asset, external_id: 'purr', symbol: 'PURR', name: 'Purr')
    ticker = create(:ticker, exchange: @exchange, base_asset: purr, quote_asset: usdc,
                             ticker: 'PURR/USDC', base: 'PURR', quote: 'USDC')

    mids_data = { 'PURR/USDC' => '2.50' }
    client.stubs(:all_mids).returns(Result::Success.new(mids_data))

    result = @exchange.get_last_price(ticker: ticker, force: true)
    assert result.success?
    assert_equal '2.50'.to_d, result.data
  end

  test 'get_bid_price returns best bid from l2_book' do
    @exchange.set_client
    client = @exchange.send(:client)

    usdc = create(:asset, external_id: 'usdc', symbol: 'USDC', name: 'USDC')
    purr = create(:asset, external_id: 'purr', symbol: 'PURR', name: 'Purr')
    ticker = create(:ticker, exchange: @exchange, base_asset: purr, quote_asset: usdc,
                             ticker: 'PURR/USDC', base: 'PURR', quote: 'USDC')

    book_data = {
      'levels' => [
        [{ 'px' => '2.49', 'sz' => '100', 'n' => 1 }],
        [{ 'px' => '2.51', 'sz' => '50', 'n' => 1 }]
      ]
    }
    client.stubs(:l2_book).returns(Result::Success.new(book_data))

    result = @exchange.get_bid_price(ticker: ticker, force: true)
    assert result.success?
    assert_equal '2.49'.to_d, result.data
  end

  test 'get_ask_price returns best ask from l2_book' do
    @exchange.set_client
    client = @exchange.send(:client)

    usdc = create(:asset, external_id: 'usdc', symbol: 'USDC', name: 'USDC')
    purr = create(:asset, external_id: 'purr', symbol: 'PURR', name: 'Purr')
    ticker = create(:ticker, exchange: @exchange, base_asset: purr, quote_asset: usdc,
                             ticker: 'PURR/USDC', base: 'PURR', quote: 'USDC')

    book_data = {
      'levels' => [
        [{ 'px' => '2.49', 'sz' => '100', 'n' => 1 }],
        [{ 'px' => '2.51', 'sz' => '50', 'n' => 1 }]
      ]
    }
    client.stubs(:l2_book).returns(Result::Success.new(book_data))

    result = @exchange.get_ask_price(ticker: ticker, force: true)
    assert result.success?
    assert_equal '2.51'.to_d, result.data
  end

  test 'market_buy raises because Hyperliquid has no native market orders' do
    @exchange.set_client

    usdc = create(:asset, external_id: 'usdc', symbol: 'USDC', name: 'USDC')
    purr = create(:asset, external_id: 'purr', symbol: 'PURR', name: 'Purr')
    ticker = create(:ticker, exchange: @exchange, base_asset: purr, quote_asset: usdc,
                             ticker: 'PURR/USDC', base: 'PURR', quote: 'USDC')

    error = assert_raises(RuntimeError) do
      @exchange.market_buy(ticker: ticker, amount: 10, amount_type: :base)
    end
    assert_match(/does not support market orders/, error.message)
  end

  test 'market_sell raises because Hyperliquid has no native market orders' do
    @exchange.set_client

    usdc = create(:asset, external_id: 'usdc', symbol: 'USDC', name: 'USDC')
    purr = create(:asset, external_id: 'purr', symbol: 'PURR', name: 'Purr')
    ticker = create(:ticker, exchange: @exchange, base_asset: purr, quote_asset: usdc,
                             ticker: 'PURR/USDC', base: 'PURR', quote: 'USDC')

    error = assert_raises(RuntimeError) do
      @exchange.market_sell(ticker: ticker, amount: 10, amount_type: :base)
    end
    assert_match(/does not support market orders/, error.message)
  end

  test 'limit_buy passes numeric size and limit_px so the gem can serialize them' do
    @exchange.set_client
    client = @exchange.send(:client)

    usdc = create(:asset, external_id: 'usdc', symbol: 'USDC', name: 'USDC')
    purr = create(:asset, external_id: 'purr', symbol: 'PURR', name: 'Purr')
    ticker = create(:ticker, exchange: @exchange, base_asset: purr, quote_asset: usdc,
                             ticker: 'PURR/USDC', base: 'PURR', quote: 'USDC',
                             base_decimals: 4, price_decimals: 5)

    captured = {}
    client.define_singleton_method(:order) do |**kwargs|
      captured.merge!(kwargs)
      # Mirror the gem: float_to_wire does Float(rounded) - x, which raises
      # "String can't be coerced into Float" if non-numeric input is passed.
      [kwargs[:size], kwargs[:limit_px]].each { |x| (Float(format('%.8f', x)) - x).abs }
      Result::Success.new('status' => 'ok',
                          'response' => { 'data' => { 'statuses' => [{ 'resting' => { 'oid' => 123 } }] } })
    end

    @exchange.stubs(:dry_run?).returns(false)
    result = @exchange.limit_buy(ticker: ticker, amount: BigDecimal('1.2345'),
                                 amount_type: :base, price: BigDecimal('0.45678'))

    assert result.success?, "expected success, got #{result.inspect}; captured=#{captured.inspect}"
    assert_kind_of Numeric, captured[:size]
    assert_kind_of Numeric, captured[:limit_px]
  end

  # == adjusted_price: Hyperliquid tick size (<=5 significant figures, <= 8 - szDecimals decimals) ==

  test 'adjusted_price floors a >$10 price to 5 significant figures (HYPE regression)' do
    ticker = hyperliquid_ticker(base_decimals: 2)
    input = BigDecimal('66.60455877')
    price = ticker.adjusted_price(price: input)

    assert_equal BigDecimal('66.604'), price
    assert price <= input, 'a buy limit must not be rounded up past the requested price'
    assert_operator significant_figures(price), :<=, 5
  end

  test 'adjusted_price floors a mid-range price to 5 significant figures' do
    ticker = hyperliquid_ticker(base_decimals: 1)
    price = ticker.adjusted_price(price: BigDecimal('3.50882766'))

    assert_equal BigDecimal('3.5088'), price
    assert_operator significant_figures(price), :<=, 5
  end

  test 'adjusted_price keeps a sub-$1 price at 5 significant figures' do
    ticker = hyperliquid_ticker(base_decimals: 0)
    price = ticker.adjusted_price(price: BigDecimal('0.451865682'))

    assert_equal BigDecimal('0.45186'), price
    assert_operator significant_figures(price), :<=, 5
  end

  test 'adjusted_price respects the 8 - szDecimals decimal cap for tiny prices' do
    ticker = hyperliquid_ticker(base_decimals: 0)
    price = ticker.adjusted_price(price: BigDecimal('0.0233952813'))

    assert_equal BigDecimal('0.023395'), price
    assert_operator significant_figures(price), :<=, 5
    assert_operator price.to_s('F').split('.').last.length, :<=, 8
  end

  test 'adjusted_price floors a >=$10k price to a valid integer price' do
    ticker = hyperliquid_ticker(base_decimals: 2)
    price = ticker.adjusted_price(price: BigDecimal('123456.78'))

    assert_equal BigDecimal('123456'), price
    assert_kind_of BigDecimal, price # integers are always valid on Hyperliquid
  end

  test 'adjusted_price with :ceil never returns below the requested price' do
    ticker = hyperliquid_ticker(base_decimals: 2)
    input = BigDecimal('66.60455877')
    price = ticker.adjusted_price(price: input, method: :ceil)

    assert_equal BigDecimal('66.605'), price
    assert_operator price, :>=, input
  end

  test 'adjusted_price handles magnitude rollover at the 5-sig-fig boundary' do
    ticker = hyperliquid_ticker(base_decimals: 2)
    price = ticker.adjusted_price(price: BigDecimal('9999.999'), method: :round)

    assert_equal BigDecimal('10000'), price
  end

  test 'adjusted_price returns zero unchanged' do
    ticker = hyperliquid_ticker(base_decimals: 2)
    assert_equal BigDecimal('0'), ticker.adjusted_price(price: BigDecimal('0'))
  end

  test 'limit_buy sends a tick-size-valid price to the exchange (<=5 sig figs)' do
    @exchange.set_client
    client = @exchange.send(:client)
    ticker = hyperliquid_ticker(base_decimals: 2)

    captured = {}
    client.define_singleton_method(:order) do |**kwargs|
      captured.merge!(kwargs)
      Result::Success.new('status' => 'ok',
                          'response' => { 'data' => { 'statuses' => [{ 'resting' => { 'oid' => 123 } }] } })
    end

    @exchange.stubs(:dry_run?).returns(false)
    result = @exchange.limit_buy(ticker: ticker, amount: BigDecimal('0.15'),
                                 amount_type: :base, price: BigDecimal('66.60455877'))

    assert result.success?, "expected success, got #{result.inspect}"
    assert_equal BigDecimal('66.604'), captured[:limit_px].to_d
    assert_operator significant_figures(captured[:limit_px]), :<=, 5
  end

  # == set_limit_order: amount_type conversion (Hyperliquid's order API takes size in base units) ==

  test 'limit_buy converts a quote amount to base size before ordering' do
    @exchange.set_client
    client = @exchange.send(:client)
    ticker = hyperliquid_ticker(base_decimals: 4)

    captured = capture_order(client)
    @exchange.stubs(:dry_run?).returns(false)

    # Buy 10 USDC worth at a 50 limit => 0.2 HYPE of size, not 10.
    result = @exchange.limit_buy(ticker: ticker, amount: BigDecimal('10'),
                                 amount_type: :quote, price: BigDecimal('50'))

    assert result.success?, "expected success, got #{result.inspect}"
    assert_equal BigDecimal('0.2'), captured[:size].to_d
    assert_equal BigDecimal('50'), captured[:limit_px].to_d
  end

  test 'limit_sell converts a quote amount to base size before ordering' do
    @exchange.set_client
    client = @exchange.send(:client)
    ticker = hyperliquid_ticker(base_decimals: 4)

    captured = capture_order(client)
    @exchange.stubs(:dry_run?).returns(false)

    result = @exchange.limit_sell(ticker: ticker, amount: BigDecimal('10'),
                                  amount_type: :quote, price: BigDecimal('50'))

    assert result.success?, "expected success, got #{result.inspect}"
    assert_equal BigDecimal('0.2'), captured[:size].to_d
  end

  test 'limit_buy sends a base amount as size unchanged' do
    @exchange.set_client
    client = @exchange.send(:client)
    ticker = hyperliquid_ticker(base_decimals: 4)

    captured = capture_order(client)
    @exchange.stubs(:dry_run?).returns(false)

    result = @exchange.limit_buy(ticker: ticker, amount: BigDecimal('0.15'),
                                 amount_type: :base, price: BigDecimal('50'))

    assert result.success?, "expected success, got #{result.inspect}"
    assert_equal BigDecimal('0.15'), captured[:size].to_d
  end

  test 'set_limit_order fails gracefully when the adjusted price is not positive' do
    @exchange.set_client
    client = @exchange.send(:client)
    ticker = hyperliquid_ticker(base_decimals: 4)

    client.define_singleton_method(:order) { |**_kwargs| raise 'order should not be called' }
    @exchange.stubs(:dry_run?).returns(false)

    result = @exchange.limit_buy(ticker: ticker, amount: BigDecimal('10'),
                                 amount_type: :quote, price: BigDecimal('0'))

    assert result.failure?
    assert_match(/price/i, result.errors.join)
  end

  private

  def capture_order(client)
    captured = {}
    client.define_singleton_method(:order) do |**kwargs|
      captured.merge!(kwargs)
      Result::Success.new('status' => 'ok',
                          'response' => { 'data' => { 'statuses' => [{ 'resting' => { 'oid' => 123 } }] } })
    end
    captured
  end

  def hyperliquid_ticker(base_decimals:)
    usdc = create(:asset, external_id: 'usdc', symbol: 'USDC', name: 'USDC')
    hype = create(:asset, external_id: 'hype', symbol: 'HYPE', name: 'Hype')
    create(:ticker, exchange: @exchange, base_asset: hype, quote_asset: usdc,
                    ticker: 'HYPE/USDC', base: 'HYPE', quote: 'USDC',
                    base_decimals: base_decimals, price_decimals: 5)
  end

  def significant_figures(value)
    digits = value.to_d.abs.to_s('F').delete('.').sub(/^0+/, '').sub(/0+$/, '')
    [digits.length, 1].max
  end
end
