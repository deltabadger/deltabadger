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
    # Not '170131': the client hands us retMsg with the code already stripped, so a bare code
    # can never appear in a message — it only added substring-collision risk.
    assert_includes errors[:insufficient_funds], 'Insufficient balance'
    assert_not_includes errors[:insufficient_funds], '170131'
    # Messages only. The retCode digits live in INVALID_KEY_CODES, because this list is
    # substring-matched against error text and a bare number matches any id that contains it.
    assert_includes errors[:invalid_key], 'Invalid API-key, IP, or permissions for action.'
    assert_not_includes errors[:invalid_key], '10003'
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

  test 'parse_order_data ends a cancelled, partially filled order as cancelled with its executed amounts' do
    ticker = create(:ticker, exchange: @exchange, base_symbol: 'BTC', quote_symbol: 'USDT', ticker: 'BTCUSDT')
    # Shape of a v5 /order/history row for a limit buy cancelled after a partial fill.
    order_data = {
      'orderId' => '1790915566373497344', 'orderLinkId' => '', 'symbol' => 'BTCUSDT', 'side' => 'Buy',
      'orderType' => 'Limit', 'price' => '62795.00', 'qty' => '0.000159', 'avgPrice' => '62795',
      'cumExecQty' => '0.000061', 'cumExecValue' => '3.830495', 'cumExecFee' => '0.000000061',
      'leavesQty' => '0', 'leavesValue' => '0', 'orderStatus' => 'PartiallyFilledCanceled',
      'cancelType' => 'CancelByUser', 'rejectReason' => 'EC_NoError', 'timeInForce' => 'GTC',
      'createdTime' => '1715700000000', 'updatedTime' => '1715700060000'
    }

    parsed = @exchange.send(:parse_order_data, '1790915566373497344', order_data)

    assert_equal :cancelled, parsed[:status]
    assert_equal ticker, parsed[:ticker]
    assert_equal BigDecimal('0.000061'), parsed[:amount_exec]
    assert_equal BigDecimal('3.830495'), parsed[:quote_amount_exec]
    assert_equal BigDecimal('0.000159'), parsed[:amount]
    assert_empty parsed[:error_messages]
  end

  # == get_balances ==
  # availableToWithdraw is deprecated for unified accounts and comes back blank, which read as 0.

  def stub_bybit_wallet(coin)
    ticker = create(:ticker, exchange: @exchange, base_symbol: 'BTC', quote_symbol: 'USDT', ticker: 'BTCUSDT')
    @exchange.send(:client).stubs(:wallet_balance).returns(
      Result::Success.new({ 'retCode' => 0, 'result' => { 'list' => [{ 'coin' => [coin.merge('coin' => 'BTC')] }] } })
    )
    ticker.base_asset
  end

  test 'get_balances reads free as wallet balance less locked and borrowed' do
    btc = stub_bybit_wallet('walletBalance' => '1.5', 'locked' => '0.5', 'spotBorrow' => '0.2',
                            'availableToWithdraw' => '', 'free' => '')

    result = with_dry_run(false) { @exchange.get_balances(asset_ids: [btc.id]) }

    assert_predicate result, :success?
    assert_equal({ free: BigDecimal('0.8'), locked: BigDecimal('0.5') }, result.data[btc.id])
  end

  test 'get_balances never reads free below zero' do
    btc = stub_bybit_wallet('walletBalance' => '0.1', 'locked' => '0.1', 'spotBorrow' => '0.2')

    result = with_dry_run(false) { @exchange.get_balances(asset_ids: [btc.id]) }

    assert_equal 0, result.data[btc.id][:free]
  end

  test 'get_balances falls back to the legacy field when walletBalance is missing' do
    btc = stub_bybit_wallet('availableToWithdraw' => '2', 'locked' => '0')

    result = with_dry_run(false) { @exchange.get_balances(asset_ids: [btc.id]) }

    assert_equal BigDecimal('2'), result.data[btc.id][:free]
  end

  # == get_ledger ==
  # A partial Success lets AccountTransactionSync move its watermark past the rows it never got.

  def stub_bybit_ledger_client
    hm_client = mock('honeymaker_client')
    Honeymaker.stubs(:client).returns(hm_client)
    empty = Result::Success.new({ 'retCode' => 0, 'result' => { 'list' => [], 'rows' => [] } })
    hm_client.stubs(:execution_list).returns(empty)
    hm_client.stubs(:deposit_records).returns(empty)
    hm_client.stubs(:withdraw_records).returns(empty)
    hm_client
  end

  test 'get_ledger succeeds when every read succeeds' do
    stub_bybit_ledger_client

    assert_predicate @exchange.get_ledger(api_key: create(:api_key, exchange: @exchange), start_time: 1.day.ago), :success?
  end

  test 'get_ledger fails when the spot trades read fails' do
    hm_client = stub_bybit_ledger_client
    hm_client.stubs(:execution_list).with(has_entry(category: 'spot')).returns(Result::Failure.new('Too many visits'))

    assert_predicate @exchange.get_ledger(api_key: create(:api_key, exchange: @exchange), start_time: 1.day.ago), :failure?
  end

  test 'get_ledger fails when the deposits read fails' do
    hm_client = stub_bybit_ledger_client
    hm_client.stubs(:deposit_records).returns(Result::Failure.new('Too many visits'))

    assert_predicate @exchange.get_ledger(api_key: create(:api_key, exchange: @exchange), start_time: 1.day.ago), :failure?
  end

  test 'get_ledger fails when the withdrawals read fails' do
    hm_client = stub_bybit_ledger_client
    hm_client.stubs(:withdraw_records).returns(Result::Failure.new('Too many visits'))

    assert_predicate @exchange.get_ledger(api_key: create(:api_key, exchange: @exchange), start_time: 1.day.ago), :failure?
  end

  # The client hands an HTTP-200 rejection over as a Success; read as an empty page, a throttle would
  # let the watermark pass trades the sync never got.
  test 'get_ledger fails when a read is throttled inside a 200 envelope' do
    hm_client = stub_bybit_ledger_client
    hm_client.stubs(:execution_list).with(has_entry(category: 'spot')).returns(
      Result::Success.new({ 'retCode' => 10_006, 'retMsg' => 'Too many visits. Exceeded the API Rate Limit.' })
    )

    result = @exchange.get_ledger(api_key: create(:api_key, exchange: @exchange), start_time: 1.day.ago)

    assert_predicate result, :failure?
    assert_match 'Too many visits', result.errors.first
  end

  test 'get_ledger fails when futures history is throttled' do
    hm_client = stub_bybit_ledger_client
    hm_client.stubs(:execution_list).with(has_entry(category: 'linear')).returns(
      Result::Success.new({ 'retCode' => 10_006, 'retMsg' => 'Too many visits. Exceeded the API Rate Limit.' })
    )

    assert_predicate @exchange.get_ledger(api_key: create(:api_key, exchange: @exchange), start_time: 1.day.ago), :failure?
  end

  # The setup asks for a Spot - Trade key without Derivatives, so futures history is denied.
  test 'get_ledger reads a denied futures history as empty' do
    hm_client = stub_bybit_ledger_client
    hm_client.stubs(:execution_list).with(has_entry(category: 'linear')).returns(
      Result::Success.new({ 'retCode' => 10_005, 'retMsg' => 'Permission denied, please check your API key permissions.' })
    )

    assert_predicate @exchange.get_ledger(api_key: create(:api_key, exchange: @exchange), start_time: 1.day.ago), :success?
  end

  test 'get_ledger fails when spot history is denied' do
    hm_client = stub_bybit_ledger_client
    hm_client.stubs(:execution_list).with(has_entry(category: 'spot')).returns(
      Result::Success.new({ 'retCode' => 10_005, 'retMsg' => 'Permission denied, please check your API key permissions.' })
    )

    assert_predicate @exchange.get_ledger(api_key: create(:api_key, exchange: @exchange), start_time: 1.day.ago), :failure?
  end
end
