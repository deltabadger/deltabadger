require 'test_helper'

class Exchanges::BingxTest < ActiveSupport::TestCase
  setup do
    @exchange = create(:bingx_exchange)
  end

  test 'coingecko_id returns bingx' do
    assert_equal 'bingx', @exchange.coingecko_id
  end

  test 'known_errors returns expected error messages' do
    errors = @exchange.known_errors
    assert errors[:insufficient_funds].present?
    assert errors[:invalid_key].present?
    assert_includes errors[:insufficient_funds], 'Insufficient balance'
    assert_includes errors[:invalid_key], 'Invalid Api-Key ID'
  end

  test 'minimum_amount_logic sizes limit orders in base and keeps the cheaper choice for market orders' do
    assert_equal :base_and_quote, @exchange.minimum_amount_logic(side: :buy, order_type: :market_order)
    assert_equal :base_and_quote_in_base, @exchange.minimum_amount_logic(side: :buy, order_type: :limit_order)
  end

  # == Order ids ==
  # Every BingX id is "<BASE>-<QUOTE>-<orderId>" (the symbol itself has a dash), stored that way
  # since the venue was added. The venue id is everything after the last dash.

  test 'get_order looks the order up by the full symbol and the venue id' do
    create(:ticker, exchange: @exchange, base_symbol: 'BTC', quote_symbol: 'USDT', ticker: 'BTC-USDT')
    raw = { 'symbol' => 'BTC-USDT', 'orderId' => 1_234_567_890, 'type' => 'LIMIT', 'side' => 'BUY',
            'origQty' => '0.001', 'executedQty' => '0.001', 'cummulativeQuoteQty' => '60',
            'status' => 'FILLED', 'price' => '60000' }
    Honeymaker::Clients::BingX.any_instance.expects(:get_order)
                              .with(symbol: 'BTC-USDT', order_id: '1234567890')
                              .returns(Result::Success.new(raw: raw))

    result = with_dry_run(false) { @exchange.get_order(order_id: 'BTC-USDT-1234567890') }

    assert_predicate result, :success?
    assert_equal 'BTC-USDT-1234567890', result.data[:order_id]
    assert_equal :closed, result.data[:status]
  end

  test 'cancel_order cancels by the full symbol and the venue id' do
    Honeymaker::Clients::BingX.any_instance.expects(:cancel_order)
                              .with(symbol: 'BTC-USDT', order_id: '1234567890')
                              .returns(Result::Success.new({ 'code' => 0, 'data' => { 'orderId' => 1_234_567_890 } }))

    result = with_dry_run(false) { @exchange.cancel_order(order_id: 'BTC-USDT-1234567890') }

    assert_predicate result, :success?
    assert_equal 'BTC-USDT-1234567890', result.data
  end

  test 'cancel_order fails when BingX refuses with HTTP 200 and a non-zero code' do
    Honeymaker::Clients::BingX.any_instance.stubs(:cancel_order)
                              .returns(Result::Success.new({ 'code' => 100_404, 'msg' => 'order not exist' }))

    result = with_dry_run(false) { @exchange.cancel_order(order_id: 'BTC-USDT-1234567890') }

    assert_predicate result, :failure?
    assert_includes result.errors.join, 'order not exist'
  end

  # == Limit orders are base-denominated ==

  test 'limit_buy with a quote amount sends a base quantity at the limit price' do
    ticker = create(:ticker, exchange: @exchange, base_symbol: 'BTC', quote_symbol: 'USDT', ticker: 'BTC-USDT',
                             price_decimals: 2, base_decimals: 6)
    captured = {}
    Honeymaker::Clients::BingX.any_instance.stubs(:place_order).with do |**kwargs|
      captured.merge!(kwargs)
    end.returns(Result::Success.new(order_id: 'BTC-USDT-1', raw: {}))

    result = with_dry_run(false) do
      @exchange.limit_buy(ticker: ticker, amount: BigDecimal('100'), amount_type: :quote, price: BigDecimal('62795'))
    end

    assert_predicate result, :success?
    adj_price = ticker.adjusted_price(price: BigDecimal('62795'))
    expected = ticker.adjusted_amount(amount: BigDecimal('100') / adj_price, amount_type: :base)
    assert_equal expected.to_d.to_s('F'), captured[:quantity]
    assert_operator expected, :positive?
  end

  test 'limit_sell with a base amount sends it unchanged' do
    ticker = create(:ticker, exchange: @exchange, base_symbol: 'BTC', quote_symbol: 'USDT', ticker: 'BTC-USDT',
                             price_decimals: 2, base_decimals: 6)
    captured = {}
    Honeymaker::Clients::BingX.any_instance.stubs(:place_order).with do |**kwargs|
      captured.merge!(kwargs)
    end.returns(Result::Success.new(order_id: 'BTC-USDT-1', raw: {}))

    with_dry_run(false) do
      @exchange.limit_sell(ticker: ticker, amount: BigDecimal('0.0012345'), amount_type: :base, price: BigDecimal('62795'))
    end

    assert_equal '0.001234', captured[:quantity]
  end

  test 'set_client creates a Honeymaker BingX client' do
    @exchange.set_client
    assert_kind_of Honeymaker::Clients::BingX, @exchange.send(:client)
  end

  test 'set_client with api_key stores the api_key' do
    api_key = create(:api_key, exchange: @exchange)
    @exchange.set_client(api_key: api_key)
    assert_equal api_key, @exchange.api_key
  end

  test 'requires_passphrase? returns false' do
    assert_equal false, @exchange.requires_passphrase?
  end

  test 'get_api_key_validity uses cancel_order for trading keys' do
    Rails.configuration.stubs(:dry_run).returns(false)
    api_key = create(:api_key, exchange: @exchange, key_type: :trading, key: 'test_key', secret: 'test_secret')

    Honeymaker::Clients::BingX.any_instance.stubs(:cancel_order).returns(
      Result::Success.new({ 'code' => 100_404, 'msg' => 'Order does not exist' })
    )
    Honeymaker::Clients::BingX.any_instance.expects(:get_raw_balances).never

    result = @exchange.get_api_key_validity(api_key: api_key)
    assert result.success?
    assert_equal true, result.data
  end

  test 'get_api_key_validity uses get_balances for withdrawal keys' do
    Rails.configuration.stubs(:dry_run).returns(false)
    api_key = create(:api_key, exchange: @exchange, key_type: :withdrawal, key: 'test_key', secret: 'test_secret')

    Honeymaker::Clients::BingX.any_instance.stubs(:get_raw_balances).returns(
      Result::Success.new({ 'code' => 0, 'data' => [] })
    )
    Honeymaker::Clients::BingX.any_instance.expects(:cancel_order).never

    result = @exchange.get_api_key_validity(api_key: api_key)
    assert result.success?
    assert_equal true, result.data
  end

  test 'get_api_key_validity returns false for invalid trading key' do
    Rails.configuration.stubs(:dry_run).returns(false)
    api_key = create(:api_key, exchange: @exchange, key_type: :trading, key: 'bad_key', secret: 'bad_secret')

    Honeymaker::Clients::BingX.any_instance.stubs(:cancel_order).returns(
      Result::Success.new({ 'code' => 100_001, 'msg' => 'Invalid Api-Key ID' })
    )

    result = @exchange.get_api_key_validity(api_key: api_key)
    assert result.success?
    assert_equal false, result.data
  end

  # ---- get_candles (the ATH ~20y lookback) ----
  #
  # BingX's kline endpoint only serves the most-recent ~1000 candles (older start_times and
  # older end_times return empty), and it returns them newest-first. get_candles must fetch
  # that single window and trim it to the requested range — a deep ATH start must keep the
  # whole window (not 0, not duplicated), sorted ascending and reaching the present.

  test 'get_candles returns the most-recent window, trimmed to the requested range and sorted' do
    ticker = create(:ticker, exchange: @exchange, base_symbol: 'BTC', quote_symbol: 'USDT')
    now = Time.now.utc
    # Most-recent ~1000 daily candles, newest-first (index 0 == today), any request shape.
    window = (0...1000).map do |i|
      t = (now - i.days).to_i * 1000
      [t, '10', '12', '9', '11', '100'] # [time_ms, open, high, low, close, volume]
    end
    client = Object.new
    client.define_singleton_method(:get_klines) { |**_kw| Result::Success.new(window) }
    @exchange.stubs(:client).returns(client)

    deep = @exchange.get_candles(ticker: ticker, start_at: 20.years.ago, timeframe: 1.day)
    assert_predicate deep, :success?
    assert_equal 1000, deep.data.size, 'a deep ATH start keeps the whole recent window'
    timestamps = deep.data.map { |c| c[0] }
    assert_equal timestamps.sort, timestamps, 'must be sorted ascending despite newest-first API order'
    assert_operator deep.data.last[0], :>=, now - 2.days, 'should reach the present'

    recent = @exchange.get_candles(ticker: ticker, start_at: 30.days.ago, timeframe: 1.day)
    assert_operator recent.data.size, :<=, 31, 'a recent start trims to the requested range'
    assert(recent.data.all? { |c| c[0] >= 30.days.ago - 1.day }, 'trimmed candles stay within range')
  end
end
