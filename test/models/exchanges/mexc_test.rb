require 'test_helper'

class Exchanges::MexcTest < ActiveSupport::TestCase
  setup do
    @exchange = create(:mexc_exchange)
  end

  test 'coingecko_id returns mxc' do
    assert_equal 'mxc', @exchange.coingecko_id
  end

  test 'known_errors returns expected error messages' do
    errors = @exchange.known_errors
    assert errors[:insufficient_funds].present?
    assert errors[:invalid_key].present?
    assert_includes errors[:insufficient_funds], 'Insufficient balance'
    assert_includes errors[:invalid_key], 'Invalid Api-Key ID.'
  end

  test 'transient_error? treats timestamp/recvWindow rejections as transient' do
    assert @exchange.transient_error?(['Timestamp for this request is outside of the recvWindow.'])
  end

  test 'minimum_amount_logic returns base_and_quote for market orders' do
    assert_equal :base_and_quote, @exchange.minimum_amount_logic(order_type: :market_order)
  end

  test 'minimum_amount_logic returns base_and_quote_in_base for limit orders' do
    assert_equal :base_and_quote_in_base, @exchange.minimum_amount_logic(order_type: :limit_order)
  end

  test 'set_client creates a Honeymaker::Clients::Mexc instance' do
    @exchange.set_client
    assert_kind_of Honeymaker::Clients::Mexc, @exchange.send(:client)
  end

  test 'set_client with api_key stores the api_key' do
    api_key = create(:api_key, exchange: @exchange)
    @exchange.set_client(api_key: api_key)
    assert_equal api_key, @exchange.api_key
  end

  test 'requires_passphrase? returns false' do
    assert_equal false, @exchange.requires_passphrase?
  end

  test 'get_api_key_validity checks canTrade for trading keys' do
    Rails.configuration.stubs(:dry_run).returns(false)
    api_key = create(:api_key, exchange: @exchange, key_type: :trading, key: 'test_key', secret: 'test_secret')

    Honeymaker::Clients::Mexc.any_instance.stubs(:account_information).returns(
      Result::Success.new({ 'canTrade' => true, 'canWithdraw' => false, 'balances' => [] })
    )

    result = @exchange.get_api_key_validity(api_key: api_key)
    assert result.success?
    assert_equal true, result.data
  end

  test 'get_api_key_validity rejects trading key without canTrade' do
    Rails.configuration.stubs(:dry_run).returns(false)
    api_key = create(:api_key, exchange: @exchange, key_type: :trading, key: 'test_key', secret: 'test_secret')

    Honeymaker::Clients::Mexc.any_instance.stubs(:account_information).returns(
      Result::Success.new({ 'canTrade' => false, 'canWithdraw' => true, 'balances' => [] })
    )

    result = @exchange.get_api_key_validity(api_key: api_key)
    assert result.success?
    assert_equal false, result.data
  end

  test 'get_api_key_validity accepts withdrawal key with successful account_information' do
    Rails.configuration.stubs(:dry_run).returns(false)
    api_key = create(:api_key, exchange: @exchange, key_type: :withdrawal, key: 'test_key', secret: 'test_secret')

    Honeymaker::Clients::Mexc.any_instance.stubs(:account_information).returns(
      Result::Success.new({ 'canTrade' => false, 'canWithdraw' => true, 'balances' => [] })
    )

    result = @exchange.get_api_key_validity(api_key: api_key)
    assert result.success?
    assert_equal true, result.data
  end
  # B8. The container has its own MEXC parser (data-api uses honeymaker's; the container cannot,
  # because honeymaker's connection takes no proxy and would put the container's own IP at MEXC).
  # Two implementations of one contract, so both are driven from the SAME captured bytes and both
  # assert its digest — a hand-edit in either repo would otherwise let them drift again.
  def mexc_exchange_info
    JSON.parse(File.read(Rails.root.join('test/fixtures/exchanges/mexc_exchange_info.json')))
  end

  test 'the MEXC fixture is the same bytes honeymaker tests against' do
    path = Rails.root.join('test/fixtures/exchanges/mexc_exchange_info.json')
    assert_equal '13a65c0c8ebde52fac4cdca9a5be9d9a18ba7b5438c496b1b30606cf52fd60d3',
                 Digest::SHA256.hexdigest(File.read(path))
    assert_equal '1', mexc_exchange_info['symbols'].first['status']
  end

  test 'get_tickers_info reads MEXC status and spot permission' do
    @exchange.set_client
    @exchange.send(:client).stubs(:exchange_information).returns(Result::Success.new(mexc_exchange_info))

    result = @exchange.get_tickers_info(force: true)

    assert result.success?
    allowed = result.data.find { |t| t[:ticker] == 'METALUSDT' }
    disallowed = result.data.find { |t| t[:ticker] == 'PALMAIUSDT' }
    assert allowed[:trading_enabled], 'status "1" plus spot permission is tradable'
    assert disallowed[:available], 'still listed'
    assert_not disallowed[:trading_enabled], 'MEXC rejects a spot order for this symbol'
  end

  # Query Order, shaped like https://mexcdevelop.github.io/apidocs/spot_v3_en/#query-order. The
  # Order Status enum lists PARTIALLY_CANCELED, which the map did not, so polling such an order raised.
  def query_order_raw(overrides = {})
    { 'symbol' => 'BTCUSDT', 'orderId' => 'C02__1', 'orderListId' => -1, 'price' => '60000', 'origQty' => '0.002',
      'executedQty' => '0', 'cummulativeQuoteQty' => '0', 'status' => 'NEW', 'timeInForce' => 'GTC',
      'type' => 'LIMIT', 'side' => 'BUY', 'time' => 1_499_827_319_559, 'updateTime' => 1_499_827_319_559,
      'isWorking' => true, 'origQuoteOrderQty' => '0.000000' }.merge(overrides)
  end

  def stub_query_order(answers)
    @exchange.send(:client).define_singleton_method(:query_order) do |symbol:, order_id:|
      answer = answers.is_a?(Hash) && !answers.key?('symbol') ? answers.fetch(order_id) : answers
      answer.is_a?(Hash) ? Result::Success.new(order_id: "#{symbol}-#{order_id}", raw: answer) : answer
    end
  end

  test 'a partially filled order cancelled on MEXC reads as cancelled with its fill' do
    stub_query_order(query_order_raw('status' => 'PARTIALLY_CANCELED', 'executedQty' => '0.001',
                                     'cummulativeQuoteQty' => '60'))

    result = with_dry_run(false) { @exchange.get_order(order_id: 'BTCUSDT-C02__1') }

    assert_predicate result, :success?
    assert_equal :cancelled, result.data[:status]
    assert_equal '0.001'.to_d, result.data[:amount_exec]
  end

  # A limit order that crossed the book fills at better than its limit. The price field is the limit,
  # so the stored price (and the average buy price built from it) was what the bot asked, not what
  # it got.
  test 'a filled limit order is priced at what it filled at, not at its limit' do
    create(:ticker, exchange: @exchange, base_symbol: 'BTC', quote_symbol: 'USDT', price_decimals: 2)
    stub_query_order(query_order_raw('status' => 'FILLED', 'price' => '60000', 'executedQty' => '0.002',
                                     'cummulativeQuoteQty' => '118'))

    result = with_dry_run(false) { @exchange.get_order(order_id: 'BTCUSDT-C02__1') }

    assert_equal '59000'.to_d, result.data[:price]
  end

  test 'an unfilled limit order keeps its limit price' do
    stub_query_order(query_order_raw)

    result = with_dry_run(false) { @exchange.get_order(order_id: 'BTCUSDT-C02__1') }

    assert_equal '60000'.to_d, result.data[:price]
  end

  # MEXC sends no LOT_SIZE or NOTIONAL filter (a live BTCUSDT carries PERCENT_PRICE_BY_SIDE only);
  # its minimums are quoteAmountPrecision ("min order amount") and baseSizePrecision ("min order
  # quantity"), https://mexcdevelop.github.io/apidocs/spot_v3_en/#exchange-information.
  test 'get_tickers_info reads the minimums MEXC sends outside its filters' do
    btc = { 'symbol' => 'BTCUSDT', 'status' => '1', 'baseAsset' => 'BTC', 'baseAssetPrecision' => 8,
            'quoteAsset' => 'USDT', 'quotePrecision' => 2, 'quoteAssetPrecision' => 2,
            'isSpotTradingAllowed' => true, 'quoteAmountPrecision' => '1', 'baseSizePrecision' => '0.000001',
            'filters' => [{ 'filterType' => 'PERCENT_PRICE_BY_SIDE', 'bidMultiplierUp' => '0.005',
                            'askMultiplierDown' => '0.005' }],
            'maxQuoteAmount' => '4000000', 'quoteAmountPrecisionMarket' => '1', 'maxQuoteAmountMarket' => '4000000' }
    @exchange.set_client
    @exchange.send(:client).stubs(:exchange_information).returns(Result::Success.new('symbols' => [btc]))

    info = @exchange.get_tickers_info(force: true).data.first

    assert_equal '0.000001'.to_d, info[:minimum_base_size]
    assert_equal 1.to_d, info[:minimum_quote_size]
    assert_equal 4_000_000.to_d, info[:maximum_quote_size]
  end

  # Wording from https://mexcdevelop.github.io/apidocs/spot_v3_en/#error-code.
  test 'the documented balance, key, permission, rate-limit and outage errors are classified' do
    assert_equal :insufficient_funds, @exchange.failure_kind(['Insufficient balance'])        # 10101
    assert_equal :insufficient_funds, @exchange.failure_kind(['Oversold'])                    # 30005
    assert_equal :insufficient_funds, @exchange.failure_kind(['Insufficient position'])       # 30004
    assert @exchange.invalid_key_error?(['API-key format invalid'])                           # 700001
    assert @exchange.invalid_key_error?(['Signature verification failed'])                    # 602
    assert @exchange.permission_error?(['No permission to access the endpoint'])              # 700007
    assert @exchange.permission_error?(['IP non white list'])                                 # 700006
    assert_not @exchange.invalid_key_error?(['No permission to access the endpoint'])
    assert @exchange.throttled_error?(['Too Many Requests'])                                  # 429
    assert @exchange.transient_error?(['service not available, please try again'])            # 503
    assert @exchange.transient_error?(['Gateway Time-out'])                                   # 504
  end

  # One order MEXC no longer has failed every get_orders batch it was in, so the bot's other open
  # orders were never polled and the gone one could never be retired.
  test 'get_orders lists an order MEXC no longer has under missing and keeps the rest' do
    stub_query_order(
      '1' => Result::Failure.new('{"code":-2013,"msg":"Order does not exist."}', data: { status: 400 }),
      '2' => query_order_raw('orderId' => '2'),
      '3' => Result::Failure.new('{"code":-2011,"msg":"Unknown order sent"}', data: { status: 400 })
    )

    result = with_dry_run(false) { @exchange.get_orders(order_ids: %w[BTCUSDT-1 BTCUSDT-2 BTCUSDT-3]) }

    assert_predicate result, :success?
    assert_equal %w[BTCUSDT-1 BTCUSDT-3], result.data[:missing]
    assert_equal :open, result.data[:orders]['BTCUSDT-2'][:status]
  end

  test 'get_orders still fails the batch on any other failure' do
    stub_query_order(Result::Failure.new('{"code":700003,"msg":"Timestamp for this request is outside of the recvWindow."}',
                                         data: { status: 400 }))

    assert_predicate with_dry_run(false) { @exchange.get_orders(order_ids: %w[BTCUSDT-1]) }, :failure?
  end

  # Kline intervals are 1m 5m 15m 30m 60m 4h 1d 1W 1M
  # (https://mexcdevelop.github.io/apidocs/spot_v3_en/#enum-definitions); a live request for 1h, 3d
  # or 1w answers -1121 "Invalid interval.".
  test 'get_candles asks for the kline intervals MEXC has' do
    ticker = create(:ticker, exchange: @exchange, base_symbol: 'BTC', quote_symbol: 'USDT')
    asked = []
    @exchange.send(:client).define_singleton_method(:candlestick_data) do |interval:, **|
      asked << interval
      Result::Success.new([])
    end

    [1.hour, 1.week, 3.days].each { |tf| @exchange.get_candles(ticker: ticker, start_at: 2.days.ago, timeframe: tf) }

    assert_equal %w[60m 1W 1d], asked.uniq
  end

  test 'get_candles builds 3-day candles from daily ones' do
    ticker = create(:ticker, exchange: @exchange, base_symbol: 'BTC', quote_symbol: 'USDT')
    day = 86_400_000
    start = (Time.now.utc.to_i / 259_200) * 259_200_000 - (3 * day)
    daily = (0..2).map { |i| [start + (i * day), (10 + i).to_s, (20 + i).to_s, (5 + i).to_s, (11 + i).to_s, '1'] }
    @exchange.send(:client).define_singleton_method(:candlestick_data) { |**| Result::Success.new(daily) }

    candles = @exchange.get_candles(ticker: ticker, start_at: Time.at(start / 1000).utc, timeframe: 3.days).data

    assert_equal [[Time.at(start / 1000).utc, 10.to_d, 22.to_d, 5.to_d, 13.to_d, 3.to_d]], candles
  end

  # Live: given a startTime alone, /api/v3/klines answers the newest 500 candles and ignores the
  # start; with startTime and endTime it answers that range, 500 at most. Stubbed the same way.
  test 'get_candles reads every window from the start to now, 500 klines at a time' do
    ticker = create(:ticker, exchange: @exchange, base_symbol: 'BTC', quote_symbol: 'USDT')
    hour = 3_600_000
    now = Time.now.utc.to_i * 1000
    open_times = ((now - (1200 * hour)) / hour * hour).step(now, hour).to_a
    asked = []
    @exchange.send(:client).define_singleton_method(:candlestick_data) do |start_time:, end_time: nil, limit: 500, **|
      asked << [start_time, end_time]
      times = end_time ? open_times.select { |t| t.between?(start_time, end_time) } : open_times
      Result::Success.new(times.last([limit, 500].min).map { |t| [t, '1', '2', '0.5', '1.5', '10'] })
    end

    candles = @exchange.get_candles(ticker: ticker, start_at: Time.at(open_times.first / 1000).utc,
                                    timeframe: 1.hour).data

    assert_equal open_times.size, candles.size
    assert_equal open_times.first / 1000, candles.first[0].to_i
    assert(asked.all? { |_, end_time| end_time })
  end

  # Ledger stubs shaped like https://mexcdevelop.github.io/apidocs/spot_v3_en/ (deposit history,
  # withdraw history, account information, account trade list).
  def stub_ledger(deposits: [], withdrawals: [], balances: [], symbols: [], trades: {})
    client = Honeymaker::Clients::Mexc.any_instance
    client.stubs(:deposit_history).returns(Result::Success.new(deposits))
    client.stubs(:withdraw_history).returns(Result::Success.new(withdrawals))
    client.stubs(:account_information).returns(Result::Success.new('canTrade' => true, 'balances' => balances))
    client.stubs(:exchange_information).returns(Result::Success.new('symbols' => symbols))
    client.stubs(:account_trade_list).returns(Result::Success.new([]))
    trades.each do |symbol, rows|
      client.stubs(:account_trade_list).with(has_entry(:symbol, symbol)).returns(Result::Success.new(rows))
    end
  end

  def ledger_api_key
    create(:api_key, exchange: @exchange, key_type: :read_only)
  end

  # applyTime is epoch milliseconds (an integer in the doc, a string in captured answers); parsing
  # it as a date string raised, so one completed withdrawal failed every sync of the account.
  test 'get_ledger books a completed withdrawal, keyed by its record id' do
    withdrawal = { 'id' => 'bb17a2d452684f00a523c015d512a341', 'txId' => nil, 'coin' => 'EOS', 'network' => 'EOS',
                   'address' => 'zzqqqqqqqqqq', 'amount' => '10', 'transferType' => 0, 'status' => 7,
                   'transactionFee' => '0.1', 'confirmNo' => nil, 'applyTime' => 1_665_300_874_000, 'remark' => '',
                   'memo' => 'MX10086', 'transHash' => '0x0ced593b8b5adc9f600334d0d7335456a7ed772ea5547beda7ffc4f33a065c',
                   'updateTime' => 1_712_134_082_000 }
    stub_ledger(withdrawals: [withdrawal, withdrawal.merge('id' => 'second', 'applyTime' => '1665300875000')])

    rows = @exchange.get_ledger(api_key: ledger_api_key, start_time: 2.days.ago).data.select { |e| e[:entry_type] == :withdrawal }

    assert_equal %w[bb17a2d452684f00a523c015d512a341 second], rows.map { |r| r[:tx_id] }
    assert_equal Time.at(1_665_300_874).utc, rows.first[:transacted_at]
    assert_equal 10.to_d, rows.first[:base_amount]
  end

  # Deposit status: 1 SMALL, 2 TIME_DELAY, 3 LARGE_DELAY, 4 PENDING, 5 SUCCESS, 6 AUDITING,
  # 7 REJECTED, 8 REFUND, 9 PRE_SUCCESS, 10 INVALID, 11 RESTRICTED, 12 COMPLETED.
  test 'get_ledger books credited deposits and skips the rest' do
    deposit = { 'amount' => '50000', 'coin' => 'EOS', 'network' => 'EOS', 'status' => 5,
                'address' => '0x20b7cf77db93d6ef1ab979c49142ec168427fdee', 'txId' => 'aa:0',
                'insertTime' => 1_659_513_342_000, 'unlockConfirm' => '10', 'confirmTimes' => '241' }
    stub_ledger(deposits: [deposit, deposit.merge('status' => '12', 'txId' => 'bb:0'),
                           deposit.merge('status' => 1, 'txId' => 'cc:0'), deposit.merge('status' => 4, 'txId' => 'dd:0')])

    rows = @exchange.get_ledger(api_key: ledger_api_key, start_time: 2.days.ago).data.select { |e| e[:entry_type] == :deposit }

    assert_equal %w[aa:0 bb:0], rows.map { |r| r[:tx_id] }
  end
end
