require 'test_helper'

class Exchanges::BybitTest < ActiveSupport::TestCase
  setup do
    @exchange = create(:bybit_exchange)
  end

  test 'coingecko_id returns bybit_spot' do
    assert_equal 'bybit_spot', @exchange.coingecko_id
  end

  test 'known_errors returns expected error messages' do
    errors = @exchange.known_errors
    assert errors[:insufficient_funds].present?
    assert errors[:invalid_key].present?
    assert_includes errors[:insufficient_funds], '170131'
    assert_includes errors[:invalid_key], '10003'
  end

  test 'minimum_amount_logic returns base_or_quote for market orders' do
    assert_equal :base_or_quote, @exchange.minimum_amount_logic(order_type: :market_order)
  end

  test 'minimum_amount_logic returns base_and_quote_in_base for limit orders' do
    # Bybit spot limit orders ship base `qty` + `price`; the order setter must size them in
    # base — never :quote — or the quote figure ships as a base quantity.
    assert_equal :base_and_quote_in_base, @exchange.minimum_amount_logic(order_type: :limit_order)
  end

  test 'set_client creates a Honeymaker::Clients::Bybit instance' do
    @exchange.set_client
    assert_kind_of Honeymaker::Clients::Bybit, @exchange.send(:client)
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

    Honeymaker::Clients::Bybit.any_instance.stubs(:cancel_order).returns(
      Result::Success.new({ 'retCode' => 110_001, 'retMsg' => 'Order does not exist' })
    )
    Honeymaker::Clients::Bybit.any_instance.expects(:wallet_balance).never

    result = @exchange.get_api_key_validity(api_key: api_key)
    assert result.success?
    assert_equal true, result.data
  end

  test 'get_api_key_validity uses wallet_balance for withdrawal keys' do
    Rails.configuration.stubs(:dry_run).returns(false)
    api_key = create(:api_key, exchange: @exchange, key_type: :withdrawal, key: 'test_key', secret: 'test_secret')

    Honeymaker::Clients::Bybit.any_instance.stubs(:wallet_balance).returns(
      Result::Success.new({ 'retCode' => 0, 'retMsg' => 'OK', 'result' => {} })
    )
    Honeymaker::Clients::Bybit.any_instance.expects(:cancel_order).never

    result = @exchange.get_api_key_validity(api_key: api_key)
    assert result.success?
    assert_equal true, result.data
  end

  test 'get_api_key_validity returns false for invalid trading key' do
    Rails.configuration.stubs(:dry_run).returns(false)
    api_key = create(:api_key, exchange: @exchange, key_type: :trading, key: 'bad_key', secret: 'bad_secret')

    Honeymaker::Clients::Bybit.any_instance.stubs(:cancel_order).returns(
      Result::Success.new({ 'retCode' => 10_003, 'retMsg' => 'Invalid apikey' })
    )

    result = @exchange.get_api_key_validity(api_key: api_key)
    assert result.success?
    assert_equal false, result.data
  end

  # == set_limit_order converts a :quote amount to base ==
  # Bybit spot limit orders ship base `qty` + `price`. A :quote amount must be converted to
  # base at the adjusted limit price, else "spend 10 USDT" ships as "buy 10 base".

  test 'set_limit_order converts a quote amount to base at the limit price' do
    ticker = create(:ticker, exchange: @exchange, base_symbol: 'BTC', quote_symbol: 'USDT',
                             price_decimals: 2, base_decimals: 6)
    client = @exchange.send(:client)
    captured = {}
    client.define_singleton_method(:create_order) do |**kwargs|
      captured.merge!(kwargs)
      Result::Success.new(order_id: "#{kwargs[:symbol]}-abc123", raw: {})
    end

    result = with_dry_run(false) do
      @exchange.send(:set_limit_order, ticker: ticker, amount: BigDecimal('10'),
                                       amount_type: :quote, side: :buy, price: BigDecimal('62795'))
    end

    assert_predicate result, :success?
    adj_price = ticker.adjusted_price(price: BigDecimal('62795'))
    expected  = ticker.adjusted_amount(amount: BigDecimal('10') / adj_price, amount_type: :base)
    assert_equal expected.to_d.to_s('F'), captured[:qty]
    assert_operator(expected.to_d * adj_price, :<=, BigDecimal('10'), 'converted base must not over-reserve quote')
    assert_equal "#{ticker.ticker}-abc123", result.data[:order_id]
  end

  test 'set_limit_order returns a failure when the adjusted price is not positive' do
    ticker = create(:ticker, exchange: @exchange, price_decimals: 0)
    @exchange.send(:client).expects(:create_order).never

    result = with_dry_run(false) do
      @exchange.send(:set_limit_order, ticker: ticker, amount: BigDecimal('10'),
                                       amount_type: :quote, side: :buy, price: BigDecimal('0.4'))
    end

    assert_predicate result, :failure?
  end
end
