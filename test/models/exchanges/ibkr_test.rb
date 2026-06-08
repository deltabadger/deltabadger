require 'test_helper'

class Exchanges::IbkrTest < ActiveSupport::TestCase
  setup do
    @exchange = create(:ibkr_exchange)
    @client = mock('clients_ibkr')
    @exchange.set_client(api_key: stub(id: 1, key: 'CONSUMER'))
    @exchange.instance_variable_set(:@client, @client)
    # Test env forces DRY_RUN=true (Exchange::Dryable intercepts orders/validity); exercise the
    # real IBKR code paths here.
    @exchange.stubs(:dry_run?).returns(false)
  end

  test 'coingecko_id is nil and known_errors never auto-bricks a key (empty :invalid_key)' do
    assert_nil @exchange.coingecko_id
    assert_equal [], @exchange.known_errors[:invalid_key]
    assert @exchange.known_errors[:transient].present?
  end

  test 'minimum_amount_logic accepts kwargs and returns :base' do
    assert_equal :base, @exchange.minimum_amount_logic
    assert_equal :base, @exchange.minimum_amount_logic(side: :buy, order_type: :market_order)
  end

  test 'does not support withdrawals and returns empty withdrawal fees' do
    refute_predicate @exchange, :supports_withdrawal?
    assert_predicate @exchange.fetch_withdrawal_fees!, :success?
  end

  test 'market_buy resolves a conid + account and places a whole-share order' do
    ticker = stub(base: 'AAPL', quote: 'USD', id: 1)
    ticker.stubs(:try).with(:conid).returns(nil)
    ticker.stubs(:adjusted_amount).returns(3)

    @client.expects(:accounts).returns(Result::Success.new({ 'accounts' => ['U1'] }))
    @client.expects(:search_contract).with(symbol: 'AAPL', currency: 'USD').returns(Result::Success.new(265_598))
    @client.expects(:place_order)
           .with(account_id: 'U1', conid: 265_598, side: :buy, quantity: 3, order_type: 'MKT', price: nil)
           .returns(Result::Success.new([{ 'order_id' => '999', 'order_status' => 'Submitted' }]))

    result = @exchange.market_buy(ticker: ticker, amount: 3, amount_type: :base)

    assert_predicate result, :success?
    assert_equal '999', result.data[:order_id]
  end

  test 'market_buy fails cleanly when the budget is below one whole share' do
    ticker = stub(base: 'BRK', quote: 'USD', id: 1)
    ticker.stubs(:adjusted_amount).returns(0)

    result = @exchange.market_buy(ticker: ticker, amount: 0, amount_type: :base)

    assert_predicate result, :failure?
    @client.expects(:place_order).never
  end

  test 'get_order maps fills to amount_exec/quote_amount_exec and returns numeric 0 when unfilled' do
    @client.expects(:order_status).with(order_id: 'o1').returns(
      Result::Success.new({ 'order_id' => 'o1', 'order_status' => 'Submitted',
                            'filledQuantity' => 0, 'avgPrice' => 0 })
    )
    data = @exchange.get_order(order_id: 'o1').data
    assert_equal 0.to_d, data[:amount_exec]
    assert_equal 0.to_d, data[:quote_amount_exec]
    assert_equal :open, data[:status]

    @client.expects(:order_status).with(order_id: 'o2').returns(
      Result::Success.new({ 'order_id' => 'o2', 'order_status' => 'Filled',
                            'filledQuantity' => 3, 'avgPrice' => 150.5 })
    )
    filled = @exchange.get_order(order_id: 'o2').data
    assert_equal 3.to_d, filled[:amount_exec]
    assert_equal (3 * 150.5).to_d, filled[:quote_amount_exec]
    assert_equal :closed, filled[:status]
  end

  test 'get_api_key_validity returns :pending_activation on an unusable (likely not-yet-activated) key' do
    Clients::Ibkr.any_instance.expects(:accounts).returns(Result::Failure.new('not authenticated'))
    result = @exchange.get_api_key_validity(api_key: stub(id: 1, key: 'C'))
    assert_predicate result, :success?
    assert_equal :pending_activation, result.data
  end

  test 'get_api_key_validity returns true when accounts come back' do
    Clients::Ibkr.any_instance.expects(:accounts).returns(Result::Success.new({ 'accounts' => ['U1'] }))
    result = @exchange.get_api_key_validity(api_key: stub(id: 1, key: 'C'))
    assert_equal true, result.data
  end

  test 'get_ledger is a safe no-op (never raises in the nightly job)' do
    result = @exchange.get_ledger(api_key: stub(id: 1), start_time: 1.day.ago)
    assert_predicate result, :success?
    assert_equal [], result.data
  end

  test 'price reads are dry-safe (no IBKR snapshot call in dry-run)' do
    @exchange.stubs(:dry_run?).returns(true)
    @client.expects(:snapshot).never
    result = @exchange.get_last_price(ticker: stub(base: 'AAPL', quote: 'USD', id: 1))
    assert_predicate result, :success?
    assert_equal BigDecimal('1'), result.data
  end

  test 'snapshot price returns a Result::Failure (never raises) on a zero/missing price' do
    ticker = stub(base: 'AAPL', quote: 'USD', id: 1)
    ticker.stubs(:try).with(:conid).returns(nil)
    @client.expects(:search_contract).returns(Result::Success.new(1))
    @client.expects(:snapshot).returns(Result::Success.new([{ 'conid' => 1, '31' => 0 }]))

    result = @exchange.get_last_price(ticker: ticker)
    assert_predicate result, :failure?
  end

  test 'market_buy fails (no fake order) when IBKR returns no order id' do
    ticker = stub(base: 'AAPL', quote: 'USD', id: 1)
    ticker.stubs(:try).with(:conid).returns(nil)
    ticker.stubs(:adjusted_amount).returns(2)
    @client.expects(:accounts).returns(Result::Success.new({ 'accounts' => ['U1'] }))
    @client.expects(:search_contract).returns(Result::Success.new(1))
    @client.expects(:place_order).returns(Result::Success.new([{ 'order_status' => 'Submitted' }]))

    result = @exchange.market_buy(ticker: ticker, amount: 2, amount_type: :base)
    assert_predicate result, :failure?
  end
end
