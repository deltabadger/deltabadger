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
end
