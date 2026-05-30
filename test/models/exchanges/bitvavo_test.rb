require 'test_helper'

class Exchanges::BitvavoTest < ActiveSupport::TestCase
  setup do
    @exchange = create(:bitvavo_exchange)
  end

  test 'coingecko_id returns bitvavo' do
    assert_equal 'bitvavo', @exchange.coingecko_id
  end

  test 'known_errors returns expected error messages' do
    errors = @exchange.known_errors
    assert errors[:insufficient_funds].present?
    assert errors[:invalid_key].present?
    assert_includes errors[:insufficient_funds], 'Insufficient funds.'
    assert_includes errors[:invalid_key], 'Invalid API key.'
  end

  test 'minimum_amount_logic returns base_or_quote' do
    assert_equal :base_or_quote, @exchange.minimum_amount_logic
  end

  test 'set_client creates a Honeymaker::Clients::Bitvavo instance' do
    @exchange.set_client
    assert_kind_of Honeymaker::Clients::Bitvavo, @exchange.send(:client)
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

    Honeymaker::Clients::Bitvavo.any_instance.stubs(:cancel_order).returns(
      Result::Failure.new('Order not found')
    )
    Honeymaker::Clients::Bitvavo.any_instance.expects(:get_raw_balance).never

    result = @exchange.get_api_key_validity(api_key: api_key)
    assert result.success?
    assert_equal true, result.data
  end

  test 'get_api_key_validity uses balance for withdrawal keys' do
    Rails.configuration.stubs(:dry_run).returns(false)
    api_key = create(:api_key, exchange: @exchange, key_type: :withdrawal, key: 'test_key', secret: 'test_secret')

    Honeymaker::Clients::Bitvavo.any_instance.stubs(:get_raw_balance).returns(
      Result::Success.new([{ 'symbol' => 'BTC', 'available' => '0.5' }])
    )
    Honeymaker::Clients::Bitvavo.any_instance.expects(:cancel_order).never

    result = @exchange.get_api_key_validity(api_key: api_key)
    assert result.success?
    assert_equal true, result.data
  end

  test 'get_api_key_validity returns false for invalid trading key' do
    Rails.configuration.stubs(:dry_run).returns(false)
    api_key = create(:api_key, exchange: @exchange, key_type: :trading, key: 'bad_key', secret: 'bad_secret')

    Honeymaker::Clients::Bitvavo.any_instance.stubs(:cancel_order).returns(
      Result::Failure.new('Invalid API key.', data: { status: 401 })
    )

    result = @exchange.get_api_key_validity(api_key: api_key)
    assert result.success?
    assert_equal false, result.data
  end

  # == get_tickers_info decimal mapping ==
  # Bitvavo deprecated pricePrecision (now null for all markets). Price precision
  # is governed by tickSize (always a power of ten), base/quote amounts by
  # quantityDecimals/notionalDecimals.

  test 'get_tickers_info maps decimals from tickSize/quantityDecimals/notionalDecimals' do
    product = {
      'market' => 'BTC-EUR', 'status' => 'trading', 'orderTypes' => %w[market limit],
      'minOrderInBaseAsset' => '0.0001', 'minOrderInQuoteAsset' => '5',
      'pricePrecision' => nil, 'tickSize' => '1.00',
      'quantityDecimals' => 8, 'notionalDecimals' => 2
    }
    Honeymaker::Clients::Bitvavo.any_instance.stubs(:get_markets).returns(Result::Success.new([product]))

    info = @exchange.get_tickers_info(force: true).data.first
    assert_equal 'BTC-EUR', info[:ticker]
    assert_equal 8, info[:base_decimals]
    assert_equal 2, info[:quote_decimals]
    assert_equal 0, info[:price_decimals] # tickSize "1.00" -> whole-number prices
  end

  test 'get_tickers_info derives price_decimals from a fractional tickSize' do
    product = {
      'market' => 'ADA-EUR', 'status' => 'trading', 'orderTypes' => %w[market limit],
      'minOrderInBaseAsset' => '0.1', 'minOrderInQuoteAsset' => '5',
      'pricePrecision' => nil, 'tickSize' => '0.0000100',
      'quantityDecimals' => 6, 'notionalDecimals' => 2
    }
    Honeymaker::Clients::Bitvavo.any_instance.stubs(:get_markets).returns(Result::Success.new([product]))

    info = @exchange.get_tickers_info(force: true).data.first
    assert_equal 5, info[:price_decimals] # decimals("0.0000100") == 5
    assert_equal 6, info[:base_decimals]
    assert_equal 2, info[:quote_decimals]
  end

  test 'get_tickers_info falls back to 8 decimals when quantity/notional fields are absent' do
    product = {
      'market' => 'NEW-EUR', 'status' => 'trading', 'orderTypes' => %w[limit],
      'minOrderInBaseAsset' => '0.1', 'minOrderInQuoteAsset' => '5',
      'pricePrecision' => nil, 'tickSize' => '0.01'
    }
    Honeymaker::Clients::Bitvavo.any_instance.stubs(:get_markets).returns(Result::Success.new([product]))

    info = @exchange.get_tickers_info(force: true).data.first
    assert_equal 8, info[:base_decimals]
    assert_equal 8, info[:quote_decimals]
    assert_equal 2, info[:price_decimals] # decimals("0.01") == 2
  end

  # == set_limit_order sends a tick-valid price ==
  # set_limit_order is unchanged; these confirm the corrected price_decimals
  # flows through Ticker#adjusted_price into the outgoing place_order payload.

  test 'set_limit_order floors the outgoing price onto a whole-number tick grid (buy)' do
    ticker = create(:ticker, exchange: @exchange, price_decimals: 0)
    client = @exchange.send(:client)
    captured = {}
    client.define_singleton_method(:place_order) do |**kwargs|
      captured.merge!(kwargs)
      Result::Success.new(order_id: "#{kwargs[:market]}-abc123", raw: { 'orderId' => 'abc123', 'market' => kwargs[:market] })
    end

    result = with_dry_run(false) do
      @exchange.send(:set_limit_order, ticker: ticker, amount: BigDecimal('0.5'),
                                       amount_type: :base, side: :buy, price: BigDecimal('95234.56'))
    end

    assert_predicate result, :success?
    assert_equal '95234.0', captured[:price] # tickSize 1.00 -> whole-number price (BigDecimal renders one trailing zero)
    assert_equal "#{ticker.ticker}-abc123", result.data[:order_id]
  end

  test 'set_limit_order floors the outgoing price to price_decimals (sell)' do
    ticker = create(:ticker, exchange: @exchange, price_decimals: 5)
    client = @exchange.send(:client)
    captured = {}
    client.define_singleton_method(:place_order) do |**kwargs|
      captured.merge!(kwargs)
      Result::Success.new(order_id: "#{kwargs[:market]}-abc123", raw: { 'orderId' => 'abc123', 'market' => kwargs[:market] })
    end

    result = with_dry_run(false) do
      @exchange.send(:set_limit_order, ticker: ticker, amount: BigDecimal('1.0'),
                                       amount_type: :base, side: :sell, price: BigDecimal('1.234567'))
    end

    assert_predicate result, :success?
    assert_equal '1.23456', captured[:price]
    assert_equal "#{ticker.ticker}-abc123", result.data[:order_id]
  end

  # == order-placement response parsing (Bug C) ==
  # Honeymaker's client#place_order returns { order_id: "MARKET-<id>", raw: {...} }
  # (symbol keys). set_limit_order/set_market_order must read result.data[:order_id],
  # not dig_or_raise(result.data, 'orderId') which raised KeyError on success.

  test 'set_market_order returns the order_id from the place_order result' do
    ticker = create(:ticker, exchange: @exchange)
    client = @exchange.send(:client)
    captured = {}
    client.define_singleton_method(:place_order) do |**kwargs|
      captured.merge!(kwargs)
      Result::Success.new(order_id: "#{kwargs[:market]}-abc123", raw: { 'orderId' => 'abc123', 'market' => kwargs[:market] })
    end

    result = with_dry_run(false) do
      @exchange.send(:set_market_order, ticker: ticker, amount: BigDecimal('25'),
                                        amount_type: :quote, side: :buy)
    end

    assert_predicate result, :success?
    assert_equal 'market', captured[:order_type]
    assert_equal "#{ticker.ticker}-abc123", result.data[:order_id]
  end

  # == get_ledger uses get_raw_balance (not the nonexistent get_balance) ==

  test 'get_ledger discovers markets via get_raw_balance and parses trades' do
    api_key = create(:api_key, exchange: @exchange, key_type: :trading, key: 'k', secret: 's')
    client_class = Honeymaker::Clients::Bitvavo

    client_class.any_instance.stubs(:get_raw_balance).returns(
      Result::Success.new([{ 'symbol' => 'BTC', 'available' => '0.5', 'inOrder' => '0' }])
    )
    client_class.any_instance.stubs(:get_markets).returns(
      Result::Success.new([{ 'market' => 'BTC-EUR', 'base' => 'BTC', 'quote' => 'EUR' }])
    )
    client_class.any_instance.stubs(:get_trades).returns(
      Result::Success.new([{ 'side' => 'buy', 'amount' => '0.1', 'price' => '50000',
                             'feeCurrency' => 'EUR', 'fee' => '0.5', 'id' => 't1',
                             'timestamp' => 1_700_000_000_000 }])
    )
    client_class.any_instance.stubs(:get_deposit_history).returns(Result::Success.new([]))
    client_class.any_instance.stubs(:get_withdrawal_history).returns(Result::Success.new([]))

    result = nil
    assert_nothing_raised { result = @exchange.get_ledger(api_key: api_key) }
    assert_predicate result, :success?
    assert(result.data.any? { |e| e[:entry_type] == :buy && e[:base_currency] == 'BTC' })
  end
end
