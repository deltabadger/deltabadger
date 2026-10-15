require 'test_helper'

class Exchanges::BinanceTest < ActiveSupport::TestCase
  setup do
    @exchange = create(:binance_exchange)
    Rails.configuration.stubs(:dry_run).returns(false)
  end

  def valid_api_description(trading: false, withdrawal: false)
    {
      'ipRestrict' => true,
      'enableFixApiTrade' => false,
      'enableFixReadOnly' => false,
      'enableFutures' => false,
      'enableInternalTransfer' => false,
      'enableMargin' => false,
      'enablePortfolioMarginTrading' => false,
      'enableReading' => true,
      'enableSpotAndMarginTrading' => trading,
      'enableVanillaOptions' => false,
      'enableWithdrawals' => withdrawal,
      'permitsUniversalTransfer' => false
    }
  end

  def binance_product(symbol:, base:, quote:, status:)
    {
      'symbol' => symbol, 'baseAsset' => base, 'quoteAsset' => quote,
      'status' => status, 'quoteAssetPrecision' => 8,
      'filters' => [
        { 'filterType' => 'PRICE_FILTER', 'tickSize' => '0.01' },
        { 'filterType' => 'LOT_SIZE', 'minQty' => '0.00001', 'maxQty' => '9000', 'stepSize' => '0.00001' },
        { 'filterType' => 'NOTIONAL', 'minNotional' => '10', 'maxNotional' => '1000000' }
      ]
    }
  end

  test 'get_tickers_info marks a trading pair available and trading_enabled' do
    @exchange.set_client
    @exchange.send(:client).stubs(:exchange_information)
             .returns(Result::Success.new({ 'symbols' => [binance_product(symbol: 'BTCUSDT', base: 'BTC', quote: 'USDT', status: 'TRADING')] }))

    ticker = @exchange.get_tickers_info(force: true).data.first
    assert ticker[:available]
    assert ticker[:trading_enabled]
  end

  test 'get_tickers_info marks a non-trading pair listed but not trading_enabled' do
    @exchange.set_client
    @exchange.send(:client).stubs(:exchange_information)
             .returns(Result::Success.new({ 'symbols' => [binance_product(symbol: 'ETHUSDT', base: 'ETH', quote: 'USDT', status: 'BREAK')] }))

    ticker = @exchange.get_tickers_info(force: true).data.first
    assert ticker[:available] # still listed
    assert_not ticker[:trading_enabled] # but not trading
  end

  test 'get_api_key_validity validates trading key permissions' do
    api_key = create(:api_key, exchange: @exchange, key_type: :trading, key: 'test_key', secret: 'test_secret')

    Honeymaker::Clients::Binance.any_instance.stubs(:api_description).returns(
      Result::Success.new(valid_api_description(trading: true, withdrawal: false))
    )

    result = @exchange.get_api_key_validity(api_key: api_key)
    assert result.success?
    assert_equal true, result.data
  end

  test 'get_api_key_validity rejects trading key with withdrawal permissions' do
    api_key = create(:api_key, exchange: @exchange, key_type: :trading, key: 'test_key', secret: 'test_secret')

    Honeymaker::Clients::Binance.any_instance.stubs(:api_description).returns(
      Result::Success.new(valid_api_description(trading: false, withdrawal: true))
    )

    result = @exchange.get_api_key_validity(api_key: api_key)
    assert result.success?
    assert_equal false, result.data
  end

  test 'get_api_key_validity validates withdrawal key permissions' do
    api_key = create(:api_key, exchange: @exchange, key_type: :withdrawal, key: 'test_key', secret: 'test_secret')

    Honeymaker::Clients::Binance.any_instance.stubs(:api_description).returns(
      Result::Success.new(valid_api_description(trading: false, withdrawal: true))
    )

    result = @exchange.get_api_key_validity(api_key: api_key)
    assert result.success?
    assert_equal true, result.data
  end

  test 'get_api_key_validity rejects withdrawal key with trading permissions' do
    api_key = create(:api_key, exchange: @exchange, key_type: :withdrawal, key: 'test_key', secret: 'test_secret')

    Honeymaker::Clients::Binance.any_instance.stubs(:api_description).returns(
      Result::Success.new(valid_api_description(trading: true, withdrawal: false))
    )

    result = @exchange.get_api_key_validity(api_key: api_key)
    assert result.success?
    assert_equal false, result.data
  end

  test 'get_api_key_validity returns false for invalid key' do
    api_key = create(:api_key, exchange: @exchange, key_type: :withdrawal, key: 'bad_key', secret: 'bad_secret')

    Honeymaker::Clients::Binance.any_instance.stubs(:api_description).returns(
      Result::Failure.new('{"code":-2015,"msg":"Invalid API-key, IP, or permissions for action."}')
    )

    result = @exchange.get_api_key_validity(api_key: api_key)
    assert result.success?
    assert_equal false, result.data
  end

  # == W2a: recvWindow on signed READS ==
  # Proxy latency spikes (up to ~30s) made Binance reject signed reads with "Timestamp outside the
  # recvWindow" (default 5000ms). Reads carry the 60s max so a delayed-but-valid request isn't rejected.
  # Order PLACEMENT stays at the default so a stale placement is rejected, not executed up to 60s late.
  test 'get_order queries with a 60s recvWindow (tolerates proxy latency)' do
    @exchange.set_client
    @exchange.send(:client)
             .expects(:query_order)
             .with(symbol: 'BTCUSDT', order_id: '123', recv_window: 60_000)
             .returns(Result::Failure.new('boom'))
    @exchange.get_order(order_id: 'BTCUSDT-123')
  end

  test 'get_orders fetches with a 60s recvWindow' do
    @exchange.set_client
    @exchange.send(:client)
             .expects(:all_orders)
             .with(symbol: 'BTCUSDT', order_id: 123, limit: 1000, recv_window: 60_000)
             .returns(Result::Failure.new('boom'))
    @exchange.get_orders(order_ids: ['BTCUSDT-123'])
  end

  test 'get_balances reads with a 60s recvWindow' do
    @exchange.set_client
    @exchange.send(:client)
             .expects(:account_information)
             .with(omit_zero_balances: true, recv_window: 60_000)
             .returns(Result::Failure.new('boom'))
    @exchange.get_balances
  end

  # == W2b: network timeouts are transient (retryable), even on an exchange with no honeymaker
  # :transient patterns (Binance has none). transient_error? must NOT early-return in that case. ==
  test 'transient_error? treats network timeouts/resets as transient' do
    assert @exchange.transient_error?(['Net::ReadTimeout with #<TCPSocket:(closed)>'])
    assert @exchange.transient_error?(['Faraday::TimeoutError: read timed out'])
    assert @exchange.transient_error?(['Faraday::ConnectionFailed: connection refused'])
    assert @exchange.transient_error?(['Net::OpenTimeout: execution expired'])
    assert @exchange.transient_error?(['Errno::ECONNRESET: Connection reset by peer'])
    assert @exchange.transient_error?(['Timestamp for this request is outside of the recvWindow.'])
    assert @exchange.transient_error?(["Timestamp for this request was 1000ms ahead of the server's time."])
  end

  test 'transient_error? does NOT retry business/auth/rate-limit errors (no false positives)' do
    assert_not @exchange.transient_error?(['Invalid API-key, IP, or permissions for action'])
    assert_not @exchange.transient_error?(['Account has insufficient balance for requested action'])
    assert_not @exchange.transient_error?(['Too many requests'])
    assert_not @exchange.transient_error?(['Filter failure: MIN_NOTIONAL'])
  end

  # Honeymaker attaches the HTTP status to a failure. The adapter re-wraps the failure with the
  # venue's own message, and must keep that metadata: a 503 with a JSON body is not a rejection
  # (Exchange#ambiguous_placement_error?).
  test 'a re-wrapped placement failure keeps the status honeymaker attached' do
    Rails.configuration.stubs(:dry_run).returns(false)
    @exchange.set_client(api_key: create(:api_key, exchange: @exchange, key_type: :trading))
    ticker = stub(ticker: 'BTCUSDT')
    ticker.stubs(:adjusted_amount).returns(100.to_d)
    Honeymaker::Clients::Binance.any_instance.stubs(:new_order).returns(
      Result::Failure.new('{"code":-1001,"msg":"backend unavailable"}', data: { status: 503 })
    )

    result = @exchange.set_market_order(ticker: ticker, amount: 100, amount_type: :quote, side: :buy)

    assert_predicate result, :failure?
    assert_equal ['backend unavailable'], result.errors
    assert_equal 503, result.data[:status]
    assert @exchange.ambiguous_placement_error?(result)
  end

  # --- the tracker's reading key: the steps ask for Enable Reading and rule out Withdrawals ---------

  def reading_validity(**flags)
    Honeymaker::Clients::Binance.any_instance.stubs(:api_description)
                                .returns(Result::Success.new({ 'enableReading' => true }.merge(flags)))
    @exchange.get_read_api_key_validity(api_key: create(:api_key, exchange: @exchange, key_type: :read_only))
  end

  test 'a reading key that reads and cannot withdraw is valid' do
    assert_equal true, reading_validity('enableWithdrawals' => false).data
  end

  # "Not needed" in the steps, so not held against the key.
  test 'a reading key may also trade' do
    assert_equal true, reading_validity('enableWithdrawals' => false, 'enableSpotAndMarginTrading' => true).data
  end

  test 'a reading key that can withdraw is refused' do
    assert_equal false, reading_validity('enableWithdrawals' => true).data
  end

  test 'a reading key without Enable Reading is refused' do
    assert_equal false, reading_validity('enableReading' => false).data
  end

  test 'a rejected reading key is incorrect, an outage is inconclusive' do
    api_key = create(:api_key, exchange: @exchange, key_type: :read_only)
    Honeymaker::Clients::Binance.any_instance.stubs(:api_description)
                                .returns(Result::Failure.new('{"code":-2014,"msg":"API-key format invalid."}'))
    assert_equal false, @exchange.get_read_api_key_validity(api_key:).data

    Honeymaker::Clients::Binance.any_instance.stubs(:api_description).returns(Result::Failure.new('execution expired'))
    assert_predicate @exchange.get_read_api_key_validity(api_key:), :failure?
  end

  test 'the reading steps say what the check holds the key to' do
    steps = I18n.t('read_only_api.binance.instructions').map { |step| step[:text_html] }.join(' ')

    assert_includes steps, '<b>Enable Reading</b>'
    assert_includes steps, 'Do not enable Withdrawals'
    assert_not_includes steps, 'Do not enable Spot'
  end

  # Each walk stops after 100 pages; one that has not reached the end by then must not pass as complete.
  test 'get_orders fails when 100 pages do not reach the orders' do
    @exchange.set_client
    @exchange.send(:client).expects(:all_orders).times(100)
             .returns(Result::Success.new((1000..1999).map { |id| { raw: { 'orderId' => id } } }))

    assert @exchange.get_orders(order_ids: ['BTCUSDT-123']).failure?
  end

  def raw_order(id)
    { raw: { 'orderId' => id, 'symbol' => 'BTCUSDT', 'status' => 'NEW', 'type' => 'LIMIT', 'side' => 'BUY',
             'price' => '100', 'origQty' => '0.1', 'executedQty' => '0', 'cummulativeQuoteQty' => '0',
             'origQuoteOrderQty' => '0' } }
  end

  # allOrders answers 1000 orders from the asked id up: the next page starts past the last one.
  test 'get_orders walks past a full page to the orders after it' do
    @exchange.set_client
    client = @exchange.send(:client)
    client.expects(:all_orders).with(symbol: 'BTCUSDT', order_id: 123, limit: 1000, recv_window: 60_000)
          .returns(Result::Success.new((123..1122).map { |id| raw_order(id) }))
    client.expects(:all_orders).with(symbol: 'BTCUSDT', order_id: 5000, limit: 1000, recv_window: 60_000)
          .returns(Result::Success.new([raw_order(5000)]))

    result = @exchange.get_orders(order_ids: %w[BTCUSDT-123 BTCUSDT-5000])
    assert result.success?
    assert_equal %w[BTCUSDT-123 BTCUSDT-5000], result.data[:orders].keys.sort
  end

  # A short page is the end: an id the venue does not return is left out after one call, not asked 100 times.
  test 'get_orders stops at a short page without an id it never saw' do
    @exchange.set_client
    @exchange.send(:client).expects(:all_orders).once.returns(Result::Success.new([raw_order(124)]))

    result = @exchange.get_orders(order_ids: %w[BTCUSDT-123 BTCUSDT-124])
    assert result.success?
    assert_equal %w[BTCUSDT-124], result.data[:orders].keys
  end

  def five_orders(**fields)
    (1..5).map { |id| { order_id: "BTCUSDT-#{id}", **fields } }
  end

  def btc_trade(order_id)
    { 'symbol' => 'BTCUSDT', 'orderId' => order_id, 'price' => '100', 'qty' => '0.1', 'quoteQty' => '10',
      'commission' => '0', 'commissionAsset' => 'BNB', 'isBuyer' => true, 'isMaker' => false, 'time' => 2 }
  end

  test 'the trades walk fails when 100 pages do not cover the orders' do
    @exchange.set_client
    @exchange.send(:client).expects(:account_trade_list).times(100)
             .returns(Result::Success.new(Array.new(1000) { { 'orderId' => 999, 'time' => 1 } }))

    result = @exchange.send(:get_aggregated_trades_for_orders, five_orders(amount: 0.1.to_d))
    assert result.failure?
  end

  test 'the trades walk stops once quote-sized orders are covered' do
    page = (1..5).map { |id| btc_trade(id) } + Array.new(995) { { 'orderId' => 999, 'time' => 1 } }
    @exchange.set_client
    @exchange.send(:client).expects(:account_trade_list).once.returns(Result::Success.new(page))

    orders = five_orders(amount: nil, quote_amount: 10.to_d, amount_exec_excl_commission: 0.1.to_d)
    result = @exchange.send(:get_aggregated_trades_for_orders, orders)
    assert result.success?
    assert_equal 0.1.to_d, result.data['BTCUSDT-3'][:amount]
  end
end
