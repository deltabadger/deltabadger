require 'test_helper'

class Exchanges::BitgetTest < ActiveSupport::TestCase
  setup do
    @exchange = create(:bitget_exchange)
  end

  test 'coingecko_id returns bitget' do
    assert_equal 'bitget', @exchange.coingecko_id
  end

  test 'known_errors includes insufficient_funds and invalid_key' do
    errors = @exchange.known_errors
    assert errors[:insufficient_funds].present?
    assert errors[:invalid_key].present?
    assert_kind_of Array, errors[:insufficient_funds]
    assert_kind_of Array, errors[:invalid_key]
  end

  # 40008 is a stalled request, not a broken bot: retry it like Binance's -1021 instead of
  # failing the run loudly. Observed in production 2026-08-16 while the UK exchange proxy hung.
  test 'transient_error? recognises an expired request timestamp' do
    assert @exchange.transient_error?(
      ['{"code":"40008","msg":"Request timestamp expired","requestTime":1786893362617,"data":null}']
    )
  end

  # Bodies as Bitget answers them over HTTP 400 (https://www.bitget.com/docs/classic/error-code/restapi).
  test 'a wrong secret or passphrase reads as an invalid key' do
    ['{"code":"40009","msg":"sign signature error"}', '{"code":"40012","msg":"apikey/password is incorrect"}',
     '{"code":"40036","msg":"passphrase is error"}', '{"code":"40041","msg":"User\'s ApiKey does not exist"}',
     '{"code":"40037","msg":"Apikey does not exist"}'].each do |body|
      assert @exchange.invalid_key_error?([body]), body
    end
  end

  test 'a missing scope reads as a permission error, not an invalid key' do
    body = '{"code":"40014","msg":"Incorrect permissions, need spot order write permissions"}'
    assert @exchange.permission_error?([body])
    assert_not @exchange.invalid_key_error?([body])
    assert @exchange.permission_error?(['{"code":"40025","msg":"The user does not have this permission"}'])
  end

  test 'a 429 reads as throttled and a venue-side hiccup as transient' do
    assert @exchange.throttled_error?(['{"code":"429","msg":"Too Many Requests"}'])
    assert @exchange.transient_error?(['{"code":"40015","msg":"System is abnormal, please try again later"}'])
    assert @exchange.transient_error?(['{"code":"40010","msg":"Request timed out"}'])
    assert_not @exchange.placement_transient_error?(['{"code":"40010","msg":"Request timed out"}'])
  end

  test 'get_api_key_validity rejects a key whose passphrase is wrong' do
    Rails.configuration.stubs(:dry_run).returns(false)
    api_key = create(:api_key, exchange: @exchange, key_type: :trading, key: 'k', secret: 's', raw_passphrase: 'wrong')
    Honeymaker::Clients::Bitget.any_instance.stubs(:cancel_order).returns(
      Result::Failure.new('{"code":"40012","msg":"apikey/password is incorrect","data":null}', data: { status: 400 })
    )

    assert_equal false, @exchange.get_api_key_validity(api_key: api_key).data
  end

  test 'transient_error? does not swallow an ordinary rejection' do
    assert_not @exchange.transient_error?(['{"code":"43012","msg":"Insufficient balance"}'])
  end

  # 40008 is rejected during signature verification, so the order never reached the book —
  # re-placing it next interval cannot double-buy. Without this, a placement rejected by a
  # stalled proxy still writes a phantom `failed` transaction and emails the user.
  test 'placement_transient_error? treats an expired timestamp as a pre-trade rejection' do
    assert @exchange.placement_transient_error?(['{"code":"40008","msg":"Request timestamp expired"}'])
  end

  test 'placement_transient_error? stays false for an ambiguous failure' do
    assert_not @exchange.placement_transient_error?(['{"code":"43012","msg":"Insufficient balance"}'])
  end

  test 'minimum_amount_logic returns base_or_quote for market orders' do
    assert_equal :base_or_quote, @exchange.minimum_amount_logic(order_type: :market_order)
  end

  test 'minimum_amount_logic returns base_and_quote_in_base for limit orders' do
    # Bitget limit orders are base-denominated (size + price, no quoteSize); the order
    # setter must size them in base — never :quote — or the quote figure ships as a base
    # quantity (-> 43012 Insufficient balance).
    assert_equal :base_and_quote_in_base, @exchange.minimum_amount_logic(order_type: :limit_order)
  end

  test 'requires_passphrase? returns true' do
    assert_predicate @exchange, :requires_passphrase?
  end

  test 'set_client creates Honeymaker::Clients::Bitget instance' do
    api_key = stub(key: 'test_key', secret: 'test_secret', passphrase: 'test_pass')
    @exchange.set_client(api_key: api_key)
    assert_kind_of Honeymaker::Clients::Bitget, @exchange.instance_variable_get(:@client)
  end

  test 'set_client sets api_key reader' do
    api_key = stub(key: 'test_key', secret: 'test_secret', passphrase: 'test_pass')
    @exchange.set_client(api_key: api_key)
    assert_equal api_key, @exchange.api_key
  end

  test 'set_client handles nil api_key' do
    @exchange.set_client(api_key: nil)
    assert_nil @exchange.api_key
    assert_kind_of Honeymaker::Clients::Bitget, @exchange.instance_variable_get(:@client)
  end

  test 'get_api_key_validity uses cancel_order for trading keys' do
    Rails.configuration.stubs(:dry_run).returns(false)
    api_key = create(:api_key, exchange: @exchange, key_type: :trading, key: 'test_key', secret: 'test_secret',
                               raw_passphrase: 'test_pass')

    Honeymaker::Clients::Bitget.any_instance.stubs(:cancel_order).returns(
      Result::Success.new({ 'code' => '43025', 'msg' => 'Order does not exist' })
    )
    Honeymaker::Clients::Bitget.any_instance.expects(:get_account_assets).never

    result = @exchange.get_api_key_validity(api_key: api_key)
    assert result.success?
    assert_equal true, result.data
  end

  test 'get_api_key_validity uses get_assets for withdrawal keys' do
    Rails.configuration.stubs(:dry_run).returns(false)
    api_key = create(:api_key, exchange: @exchange, key_type: :withdrawal, key: 'test_key', secret: 'test_secret',
                               raw_passphrase: 'test_pass')

    Honeymaker::Clients::Bitget.any_instance.stubs(:get_account_assets).returns(
      Result::Success.new({ 'code' => '00000', 'data' => [] })
    )
    Honeymaker::Clients::Bitget.any_instance.expects(:cancel_order).never

    result = @exchange.get_api_key_validity(api_key: api_key)
    assert result.success?
    assert_equal true, result.data
  end

  test 'get_api_key_validity returns false for invalid trading key' do
    Rails.configuration.stubs(:dry_run).returns(false)
    api_key = create(:api_key, exchange: @exchange, key_type: :trading, key: 'bad_key', secret: 'bad_secret',
                               raw_passphrase: 'test_pass')

    Honeymaker::Clients::Bitget.any_instance.stubs(:cancel_order).returns(
      Result::Success.new({ 'code' => '40014', 'msg' => 'Invalid Api Key' })
    )

    result = @exchange.get_api_key_validity(api_key: api_key)
    assert result.success?
    assert_equal false, result.data
  end

  # Production: Bitget's v2 cancel-order returns HTTP 400 for business errors, so honeymaker's
  # with_rescue wraps order-not-found (43001 订单不存在) as a Failure carrying the raw JSON body.
  # The probe getting past the permission gate means the key CAN trade ⇒ valid.
  test 'get_api_key_validity treats order-not-found Failure (HTTP 400) as a valid trading key' do
    Rails.configuration.stubs(:dry_run).returns(false)
    api_key = create(:api_key, exchange: @exchange, key_type: :trading, key: 'test_key', secret: 'test_secret',
                               raw_passphrase: 'test_pass')

    Honeymaker::Clients::Bitget.any_instance.stubs(:cancel_order).returns(
      Result::Failure.new('{"code":"43001","msg":"订单不存在","data":null}', data: { status: 400 })
    )

    result = @exchange.get_api_key_validity(api_key: api_key)
    assert result.success?
    assert_equal true, result.data
  end

  test 'get_api_key_validity treats no-trade-permission Failure (HTTP 400) as an invalid trading key' do
    Rails.configuration.stubs(:dry_run).returns(false)
    api_key = create(:api_key, exchange: @exchange, key_type: :trading, key: 'test_key', secret: 'test_secret',
                               raw_passphrase: 'test_pass')

    Honeymaker::Clients::Bitget.any_instance.stubs(:cancel_order).returns(
      Result::Failure.new(
        '{"code":"40014","msg":"Incorrect permissions, need spot order write permissions","data":null}',
        data: { status: 400 }
      )
    )

    result = @exchange.get_api_key_validity(api_key: api_key)
    assert result.success?
    assert_equal false, result.data
  end

  test 'get_api_key_validity treats 401 auth Failure as an invalid trading key' do
    Rails.configuration.stubs(:dry_run).returns(false)
    api_key = create(:api_key, exchange: @exchange, key_type: :trading, key: 'bad_key', secret: 'bad_secret',
                               raw_passphrase: 'test_pass')

    Honeymaker::Clients::Bitget.any_instance.stubs(:cancel_order).returns(
      Result::Failure.new('Invalid signature', data: { status: 401 })
    )

    result = @exchange.get_api_key_validity(api_key: api_key)
    assert result.success?
    assert_equal false, result.data
  end

  # A genuine transport failure (no JSON body, unknown code) must propagate as a Failure so the key
  # is left pending_validation (retryable) — not coerced into a true/false verdict.
  test 'get_api_key_validity propagates a genuine transport failure' do
    Rails.configuration.stubs(:dry_run).returns(false)
    api_key = create(:api_key, exchange: @exchange, key_type: :trading, key: 'test_key', secret: 'test_secret',
                               raw_passphrase: 'test_pass')

    Honeymaker::Clients::Bitget.any_instance.stubs(:cancel_order).returns(
      Result::Failure.new('Connection timed out')
    )

    result = @exchange.get_api_key_validity(api_key: api_key)
    assert result.failure?
  end

  # == set_limit_order converts a :quote amount to base ==
  # Bitget limit orders accept only base `size` + `price` (no quoteSize). A :quote amount
  # must be converted to base at the adjusted limit price, else "spend 10 USDT" ships as
  # "buy 10 BTC" -> 43012 Insufficient balance.

  test 'set_limit_order converts a quote amount to base at the limit price' do
    ticker = create(:ticker, exchange: @exchange, base_symbol: 'BTC', quote_symbol: 'USDT',
                             price_decimals: 2, base_decimals: 6)
    client = @exchange.send(:client)
    captured = {}
    client.define_singleton_method(:place_order) do |**kwargs|
      captured.merge!(kwargs)
      Result::Success.new(order_id: "#{kwargs[:symbol]}-abc123", raw: { 'orderId' => 'abc123' })
    end

    result = with_dry_run(false) do
      @exchange.send(:set_limit_order, ticker: ticker, amount: BigDecimal('10'),
                                       amount_type: :quote, side: :buy, price: BigDecimal('62795'))
    end

    assert_predicate result, :success?
    adj_price = ticker.adjusted_price(price: BigDecimal('62795'))
    expected  = ticker.adjusted_amount(amount: BigDecimal('10') / adj_price, amount_type: :base)
    assert_equal expected.to_d.to_s('F'), captured[:size]
    assert_nil captured[:quote_size], 'limit orders must not send quote_size'
    assert_operator(expected.to_d * adj_price, :<=, BigDecimal('10'), 'converted base must not over-reserve quote')
    assert_equal "#{ticker.ticker}-abc123", result.data[:order_id]
  end

  test 'set_limit_order with a base amount is unchanged (floored to base_decimals)' do
    ticker = create(:ticker, exchange: @exchange, base_symbol: 'BTC', quote_symbol: 'USDT',
                             price_decimals: 2, base_decimals: 6)
    client = @exchange.send(:client)
    captured = {}
    client.define_singleton_method(:place_order) do |**kwargs|
      captured.merge!(kwargs)
      Result::Success.new(order_id: "#{kwargs[:symbol]}-abc123", raw: { 'orderId' => 'abc123' })
    end

    with_dry_run(false) do
      @exchange.send(:set_limit_order, ticker: ticker, amount: BigDecimal('0.00016117778'),
                                       amount_type: :base, side: :buy, price: BigDecimal('62795'))
    end

    assert_equal '0.000161', captured[:size] # floored to base_decimals (6)
  end

  test 'set_limit_order returns a failure when the adjusted price is not positive' do
    ticker = create(:ticker, exchange: @exchange, price_decimals: 0)
    @exchange.send(:client).expects(:place_order).never

    result = with_dry_run(false) do
      @exchange.send(:set_limit_order, ticker: ticker, amount: BigDecimal('10'),
                                       amount_type: :quote, side: :buy, price: BigDecimal('0.4'))
    end

    assert_predicate result, :failure?
  end

  # == set_market_order sends one `size`, in the unit the side takes ==
  # Bitget's v2 place-order has no quoteSize: a market buy's `size` is quote coins, a market sell's
  # is base coins. Quote amounts went out as quoteSize with no size at all, and base amounts on a
  # buy went out as a size Bitget reads as quote.

  def capture_place_order
    captured = {}
    @exchange.send(:client).define_singleton_method(:place_order) do |**kwargs|
      captured.merge!(kwargs)
      Result::Success.new(order_id: "#{kwargs[:symbol]}-abc123", raw: { 'orderId' => 'abc123' })
    end
    captured
  end

  test 'a quote-sized market buy sends the quote amount as size' do
    ticker = create(:ticker, exchange: @exchange, base_symbol: 'BTC', quote_symbol: 'USDT', quote_decimals: 8)
    captured = capture_place_order

    with_dry_run(false) { @exchange.market_buy(ticker: ticker, amount: BigDecimal('25'), amount_type: :quote) }

    assert_equal '25.0', captured[:size]
    assert_nil captured[:quote_size]
  end

  test 'a base-sized market buy is converted to quote at the ask' do
    ticker = create(:ticker, exchange: @exchange, base_symbol: 'BTC', quote_symbol: 'USDT', quote_decimals: 8)
    @exchange.stubs(:market_price_for).with(ticker: ticker, side: :buy).returns(Result::Success.new(BigDecimal('50000')))
    captured = capture_place_order

    with_dry_run(false) { @exchange.market_buy(ticker: ticker, amount: BigDecimal('0.001'), amount_type: :base) }

    assert_equal '50.0', captured[:size]
  end

  test 'a market sell sends a base size, converting a quote amount at the bid' do
    ticker = create(:ticker, exchange: @exchange, base_symbol: 'BTC', quote_symbol: 'USDT', base_decimals: 6)
    @exchange.stubs(:market_price_for).with(ticker: ticker, side: :sell).returns(Result::Success.new(BigDecimal('50000')))
    captured = capture_place_order

    with_dry_run(false) { @exchange.market_sell(ticker: ticker, amount: BigDecimal('0.0021'), amount_type: :base) }
    assert_equal '0.0021', captured[:size]

    with_dry_run(false) { @exchange.market_sell(ticker: ticker, amount: BigDecimal('100'), amount_type: :quote) }
    assert_equal '0.002', captured[:size]
  end

  # == parse_order_status maps Bitget's v2 spot status enum ==
  # Bitget migrated the spot order status enum: the live /api/v2/spot/trade/orderInfo endpoint
  # now returns filled / partially_filled where it once returned full_fill / partial_fill. Our
  # mapping still knew only the superseded terms, so a fully-filled order returned "filled",
  # which raised RuntimeError("Unknown Bitget order status: filled") and crashed the polling job.
  # Any future enum change is surfaced loudly by the else-raise rather than mapped speculatively.

  test 'parse_order_status maps the modern v2 enum (filled, partially_filled, live)' do
    assert_equal :closed, @exchange.send(:parse_order_status, 'filled')
    assert_equal :open, @exchange.send(:parse_order_status, 'partially_filled')
    assert_equal :open, @exchange.send(:parse_order_status, 'live')
  end

  test 'parse_order_status maps pre-orderbook and cancelled states' do
    assert_equal :unknown, @exchange.send(:parse_order_status, 'init')
    assert_equal :unknown, @exchange.send(:parse_order_status, 'new')
    assert_equal :cancelled, @exchange.send(:parse_order_status, 'cancelled')
  end

  test 'parse_order_status raises on a genuinely unknown status' do
    error = assert_raises(RuntimeError) { @exchange.send(:parse_order_status, 'something_new') }
    assert_match(/Unknown Bitget order status: something_new/, error.message)
  end

  # End-to-end repro of the overnight crash: a filled order polled via get_order must normalize
  # to :closed instead of raising. This is the exact path Bot::FetchAndUpdateOrderJob /
  # Bot::FetchAndUpdateOpenOrdersJob take.
  test 'get_order normalizes a filled order to :closed without raising' do
    client = @exchange.send(:client)
    client.define_singleton_method(:get_order) do |order_id:|
      Result::Success.new(raw: {
                            'orderId' => order_id, 'symbol' => 'BTCUSDT', 'orderType' => 'market', 'side' => 'buy',
                            'price' => '0', 'priceAvg' => '62795.5', 'size' => '10', 'quoteSize' => '10',
                            'baseVolume' => '0.000159', 'quoteVolume' => '9.99', 'status' => 'filled'
                          })
    end

    result = with_dry_run(false) { @exchange.get_order(order_id: 'BTCUSDT-abc123') }

    assert_predicate result, :success?
    assert_equal :closed, result.data[:status]
  end

  # orderInfo's `size` is quote coins on a market buy; reading it as base wrote "10 BTC" onto a
  # 10 USDT buy. Shaped like the doc's example, which has no quoteSize.
  test 'get_order reads a market buy size as the quote amount' do
    raw = { 'orderId' => '1', 'symbol' => 'BTCUSDT', 'orderType' => 'market', 'side' => 'buy', 'price' => '0',
            'size' => '10.0000000000000000', 'priceAvg' => '13000', 'baseVolume' => '0.0007',
            'quoteVolume' => '9.1', 'status' => 'filled' }
    @exchange.send(:client).define_singleton_method(:get_order) { |**| Result::Success.new(raw: raw) }

    data = with_dry_run(false) { @exchange.get_order(order_id: 'BTCUSDT-1') }.data

    assert_nil data[:amount]
    assert_equal BigDecimal('10'), data[:quote_amount]
    assert_equal BigDecimal('0.0007'), data[:amount_exec]

    raw.merge!('orderType' => 'limit', 'price' => '13000', 'size' => '0.0007')
    data = with_dry_run(false) { @exchange.get_order(order_id: 'BTCUSDT-1') }.data
    assert_equal BigDecimal('0.0007'), data[:amount]
    assert_nil data[:quote_amount]
  end
  # B9. Bitget emitted `available: status == 'online'` and no trading_enabled at all — one concept
  # in two columns with opposite failure modes on the two sync paths. Every other venue reports an
  # offline pair as listed-but-not-tradable.
  test 'an offline Bitget pair is listed but not trading_enabled' do
    products = [{
      'symbol' => 'BTCUSDT', 'baseCoin' => 'BTC', 'quoteCoin' => 'USDT', 'status' => 'offline',
      'minTradeAmount' => '0.0001', 'minTradeUSDT' => '5', 'maxTradeAmount' => '1000',
      'quantityPrecision' => '4', 'quotePrecision' => '6', 'pricePrecision' => '2'
    }]
    @exchange.set_client
    @exchange.send(:client).stubs(:get_symbols).returns(Result::Success.new({ 'data' => products }))

    @exchange.send(:client).stubs(:get_tickers).returns(Result::Success.new({ 'data' => [] }))

    info = @exchange.get_tickers_info(force: true).data.first

    assert info[:available], 'an offline pair is still listed'
    assert_not info[:trading_enabled], 'but it cannot be traded'
  end

  # minTradeUSDT is in USDT on every pair (live: ETHBTC, SOLBTC and BTCEUR all carry "1"), so
  # storing it as the quote minimum made ETHBTC's minimum 1 BTC and every order on it a skip.
  test 'the quote minimum of a non-USDT pair is minTradeUSDT converted into the quote' do
    product = lambda do |symbol, base, quote|
      { 'symbol' => symbol, 'baseCoin' => base, 'quoteCoin' => quote, 'status' => 'online',
        'minTradeAmount' => '0', 'minTradeUSDT' => '1', 'maxTradeAmount' => '900000000000000000000',
        'quantityPrecision' => '4', 'quotePrecision' => '9', 'pricePrecision' => '5' }
    end
    products = [product.call('ETHBTC', 'ETH', 'BTC'), product.call('BTCEUR', 'BTC', 'EUR'),
                product.call('BTCUSDT', 'BTC', 'USDT'), product.call('XYZABC', 'XYZ', 'ABC')]
    tickers = [{ 'symbol' => 'BTCUSDT', 'lastPr' => '50000' }, { 'symbol' => 'USDTEUR', 'lastPr' => '0.9' }]
    @exchange.set_client
    @exchange.send(:client).stubs(:get_symbols).returns(Result::Success.new({ 'data' => products }))
    @exchange.send(:client).stubs(:get_tickers).returns(Result::Success.new({ 'data' => tickers }))

    minimums = @exchange.get_tickers_info(force: true).data.to_h { |info| [info[:ticker], info[:minimum_quote_size]] }

    assert_equal BigDecimal('0.00002'), minimums['ETHBTC']
    assert_equal BigDecimal('0.9'), minimums['BTCEUR']
    assert_equal BigDecimal('1'), minimums['BTCUSDT']
    assert_equal 0, minimums['XYZABC']
  end
end
