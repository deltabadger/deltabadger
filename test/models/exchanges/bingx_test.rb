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

  # Wording from https://bingx-api.github.io/docs/#/en-us/common/error-code and the place-order samples.
  test 'the documented balance, key, permission, rate-limit and outage errors are classified' do
    assert_equal :insufficient_funds,
                 @exchange.failure_kind(['BingX API error 100202: Insufficient assets, please verify the assets status'])
    assert @exchange.invalid_key_error?(['Incorrect apiKey, please check your valid api key in https://bingx.com/en/account/api'])
    assert @exchange.permission_error?(['Permission denied as the API key was created without the permission'])
    assert @exchange.permission_error?(['IP does not match IP whitelist'])
    assert_not @exchange.invalid_key_error?(['IP does not match IP whitelist'])
    assert @exchange.throttled_error?(['rate limitation'])
    assert @exchange.transient_error?(['BingX API error 100500: The current system is busy, please try again later'])
    assert @exchange.transient_error?(['Null timestamp or timestamp mismatch'])
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

  # A limit order's price field is the limit. One that crossed the book filled better than that, and
  # the stored price feeds the average buy price.
  test 'get_order prices a filled limit order at its fills, not at its limit' do
    create(:ticker, exchange: @exchange, base_symbol: 'BTC', quote_symbol: 'USDT', ticker: 'BTC-USDT', price_decimals: 2)
    raw = { 'symbol' => 'BTC-USDT', 'orderId' => 1, 'type' => 'LIMIT', 'side' => 'BUY', 'price' => '60000',
            'origQty' => '0.002', 'executedQty' => '0.002', 'cummulativeQuoteQty' => '110', 'status' => 'FILLED',
            'origQuoteOrderQty' => '0', 'avgPrice' => '55000' }
    Honeymaker::Clients::BingX.any_instance.stubs(:get_order).returns(Result::Success.new(raw: raw))

    result = with_dry_run(false) { @exchange.get_order(order_id: 'BTC-USDT-1') }

    assert_equal BigDecimal('55000'), result.data[:price]
  end

  # minQty is deprecated and stale: on BTC-USDT it is worth ~15 USDT while minNotional is 0.5, so a
  # 10 USDT order was skipped as below the minimum.
  test 'get_tickers_info takes the minimum from minNotional, not the deprecated minQty' do
    row = { 'symbol' => 'BTC-USDT', 'minQty' => 0.0001826, 'maxQty' => 18.2663756, 'minNotional' => 0.5,
            'maxNotional' => 7_588_825.16, 'maxMarketNotional' => 7_588_825.16, 'status' => 1,
            'tickSize' => 0.01, 'stepSize' => 1e-06, 'apiStateSell' => true, 'apiStateBuy' => true }
    Honeymaker::Clients::BingX.any_instance.stubs(:get_symbols)
                              .returns(Result::Success.new({ 'code' => 0, 'data' => { 'symbols' => [row] } }))

    info = @exchange.get_tickers_info(force: true).data.first

    assert_equal 0, info[:minimum_base_size]
    assert_equal BigDecimal('0.5'), info[:minimum_quote_size]
  end

  # One order BingX no longer has (100404) failed the whole batch on every poll, and missing was
  # always empty, so the stale-order sweep could never retire it.
  test 'get_orders lists an order BingX no longer has under missing' do
    create(:ticker, exchange: @exchange, base_symbol: 'BTC', quote_symbol: 'USDT', ticker: 'BTC-USDT')
    raw = { 'symbol' => 'BTC-USDT', 'orderId' => 2, 'type' => 'MARKET', 'side' => 'BUY', 'price' => '0',
            'origQty' => '0.001', 'executedQty' => '0.001', 'cummulativeQuoteQty' => '60', 'status' => 'FILLED' }
    Honeymaker::Clients::BingX.any_instance.stubs(:get_order).with(symbol: 'BTC-USDT', order_id: '1')
                              .returns(Result::Failure.new('BingX API error 100404: order not exist'))
    Honeymaker::Clients::BingX.any_instance.stubs(:get_order).with(symbol: 'BTC-USDT', order_id: '2')
                              .returns(Result::Success.new(raw: raw))

    result = with_dry_run(false) { @exchange.get_orders(order_ids: %w[BTC-USDT-1 BTC-USDT-2]) }

    assert_predicate result, :success?
    assert_equal %w[BTC-USDT-1], result.data[:missing]
    assert_equal %w[BTC-USDT-2], result.data[:orders].keys
  end

  # 100404 is also the gateway's "path not found"; read as a gone order it would let the stale-order
  # sweep abandon a live one.
  test 'get_orders fails the batch on a gateway 100404' do
    Honeymaker::Clients::BingX.any_instance.stubs(:get_order)
                              .returns(Result::Failure.new('BingX API error 100404: api path not found'))

    result = with_dry_run(false) { @exchange.get_orders(order_ids: %w[BTC-USDT-1]) }

    assert_predicate result, :failure?
  end

  test 'get_orders still fails the batch on any other failure' do
    Honeymaker::Clients::BingX.any_instance.stubs(:get_order)
                              .returns(Result::Failure.new('BingX API error 100400: miss arguments'))

    result = with_dry_run(false) { @exchange.get_orders(order_ids: %w[BTC-USDT-1]) }

    assert_predicate result, :failure?
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
    client.define_singleton_method(:get_public) { |_path, _params| Result::Success.new(window) }
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

  # Without timeZone=0 BingX anchors spot candles to UTC+8: a daily candle opened at 16:00 UTC, so a
  # day's close was the price at 16:00 UTC and its date the day before.
  test 'get_candles asks for candles on UTC boundaries' do
    ticker = create(:ticker, exchange: @exchange, base_symbol: 'BTC', quote_symbol: 'USDT', ticker: 'BTC-USDT')
    asked = []
    client = Object.new
    client.define_singleton_method(:get_public) do |path, params|
      asked << [path, params]
      Result::Success.new({ 'code' => 0, 'data' => [[1_792_108_800_000, 82_239.81, 82_533.22, 81_698.79, 82_302.01, 602.9]] })
    end
    @exchange.stubs(:client).returns(client)

    candles = @exchange.get_candles(ticker: ticker, start_at: Time.utc(2026, 10, 10), timeframe: 1.day).data

    assert_equal '/openApi/spot/v1/market/kline', asked.first[0]
    assert_equal 0, asked.first[1][:timeZone]
    assert_equal Time.utc(2026, 10, 16), candles.first[0]
  end

  # == Ledger ==

  # What BingX answers a path it does not have, captured live for /openApi/spot/v1/trade/fills and
  # /openApi/wallets/v1/capital/{deposit/hisrec,withdraw/history} — the paths honeymaker asks.
  GONE_PATH = { 'code' => 100_400, 'msg' => 'this api is not exist,please refer to the API docs https://bingx-api.github.io/docs',
                'timestamp' => 1_792_137_475_001 }.freeze

  def ledger_api_key
    create(:api_key, exchange: @exchange, key_type: :read_only)
  end

  # A client whose honeymaker ledger methods answer like the live venue (GONE_PATH) and whose signed
  # requests are served by `routes`: path => ->(params) { body }; a pair has no trades unless routed.
  def fake_ledger_client(routes, asked: [])
    routes = { TRADES => trades_route([]) }.merge(routes)
    fake = Honeymaker.client('bingx')
    %i[get_trade_fills deposit_history withdraw_history].each do |m|
      fake.define_singleton_method(m) { |**| Result::Success.new(GONE_PATH) }
    end
    fake.define_singleton_method(:get_raw_balances) do |**|
      Result::Success.new({ 'code' => 0, 'data' => { 'balances' => [{ 'asset' => 'BTC', 'free' => '1', 'locked' => '0' }] } })
    end
    fake.define_singleton_method(:get_symbols) do |**|
      Result::Success.new({ 'code' => 0, 'data' => { 'symbols' => [{ 'symbol' => 'BTC-USDT', 'status' => 1 }] } })
    end
    fake.define_singleton_method(:get_signed) do |path, params = {}|
      asked << [path, params]
      route = routes[path]
      Result::Success.new(route ? route.call(params) : GONE_PATH)
    end
    Honeymaker.stubs(:client).returns(fake)
    asked
  end

  DEPOSITS = '/openApi/api/v3/capital/deposit/hisrec'.freeze
  WITHDRAWALS = '/openApi/api/v3/capital/withdraw/history'.freeze

  # Shaped like https://bingx-api.github.io/docs/#/en-us/common/wallet-api.html (Deposit records,
  # Withdraw records): a bare array, the coin carrying its network, applyTime a date string.
  test 'get_ledger reads deposits and withdrawals from the documented paths' do
    insert = (2.days.ago.to_i * 1000)
    fake_ledger_client({
                         DEPOSITS => lambda { |params|
                           return [] unless params[:offset].zero? && insert.between?(params[:startTime], params[:endTime])

                           [{ 'amount' => '49999.00000000000000000000', 'coin' => 'USDTTRC20', 'network' => 'TRC20', 'status' => 1,
                              'address' => 'TPxB4v', 'addressTag' => '', 'txId' => '60ab1d', 'insertTime' => insert,
                              'unlockConfirm' => '2/2', 'confirmTimes' => '2/2' },
                            { 'amount' => '5', 'coin' => 'USDT', 'network' => 'TRC20', 'status' => 0, 'txId' => 'pending', 'insertTime' => insert }]
                         },
                         WITHDRAWALS => lambda { |params|
                           return [] unless params[:offset].zero? && insert.between?(params[:startTime], params[:endTime])

                           [{ 'address' => 'TRzc', 'amount' => '3500.00000000000000000000', 'applyTime' => '2023-12-14T04:05:02.000+08:00',
                              'coin' => 'USDTTRC20', 'id' => '1250000098', 'network' => 'TRC20', 'transferType' => 1,
                              'transactionFee' => '1.00000000000000000000', 'confirmNo' => 2, 'info' => '', 'txId' => 'b9b67', 'status' => 6 }]
                         }
                       })

    result = @exchange.get_ledger(api_key: ledger_api_key, start_time: 3.days.ago)

    assert_predicate result, :success?
    deposit, withdrawal = result.data.partition { |e| e[:entry_type] == :deposit }.map(&:sole)
    assert_equal ['USDT', 49_999.to_d, '60ab1d'], deposit.values_at(:base_currency, :base_amount, :tx_id)
    assert_equal ['USDT', 3500.to_d, 1.to_d, '1250000098'], withdrawal.values_at(:base_currency, :base_amount, :fee_amount, :tx_id)
    assert_equal Time.utc(2023, 12, 13, 20, 5, 2), withdrawal[:transacted_at]
  end

  # Unknown range cap: 90-day windows from BingX's opening on a first sync; within one, offset pages
  # of 1000 until a short one.
  test 'get_ledger reads every transfer since the opening, page by page' do
    times = (0...1500).map { |i| (Time.utc(2019, 6, 1).to_i * 1000) + (i * 60_000) }
    asked = fake_ledger_client({
                                 DEPOSITS => lambda { |params|
                                   rows = times.grep(params[:startTime]..params[:endTime]).drop(params[:offset]).first(params[:limit])
                                   rows.map { |t| { 'coin' => 'BTC', 'amount' => '1', 'status' => 1, 'txId' => "h#{t}", 'insertTime' => t } }
                                 },
                                 WITHDRAWALS => ->(_params) { [] }
                               })

    rows = @exchange.get_ledger(api_key: ledger_api_key).data

    assert_equal 1500, rows.map { |r| r[:tx_id] }.uniq.size
    deposit_asks = asked.select { |path, _| path == DEPOSITS }.map(&:last)
    assert_equal Time.utc(2018, 1, 1).to_i * 1000, deposit_asks.first[:startTime]
    assert(deposit_asks.all? { |p| p[:endTime] - p[:startTime] <= 90.days.in_milliseconds })
    assert_operator deposit_asks.last[:endTime], :>=, 1.minute.ago.to_i * 1000
  end

  test 'a failed transfer read fails the fetch; a key without the wallet permission reads as no transfers' do
    fake_ledger_client({ DEPOSITS => ->(_) { { 'code' => 100_410, 'msg' => 'rate limitation' } }, WITHDRAWALS => ->(_) { [] } })
    assert_predicate @exchange.get_ledger(api_key: ledger_api_key, start_time: 2.days.ago), :failure?

    denied = { 'code' => 100_004, 'msg' => 'Permission denied as the API key was created without the permission' }
    fake_ledger_client({ DEPOSITS => ->(_) { denied }, WITHDRAWALS => ->(_) { denied } })
    assert_predicate @exchange.get_ledger(api_key: ledger_api_key, start_time: 2.days.ago), :success?
  end

  TRADES = '/openApi/spot/v1/trade/myTrades'.freeze

  # myTrades shaped like https://bingx-api.github.io/docs/#/en-us/spot/trade-api.html#Query%20transaction%20details:
  # data.fills, oldest first, at most 500 of the range asked.
  def trades_route(times)
    lambda do |params|
      rows = times.each_with_index.select { |t, _| t.between?(params[:startTime], params[:endTime]) }.first([params[:limit], 500].min)
      fills = rows.map do |t, i|
        { 'symbol' => params[:symbol], 'id' => 36_767_057 + i, 'orderId' => 1_745_000 + i, 'price' => '46820.155',
          'qty' => '0.001', 'quoteQty' => '46.82', 'commission' => -0.0000465, 'commissionAsset' => 'BTC',
          'time' => t, 'isBuyer' => true, 'isMaker' => false }
      end
      { 'code' => 0, 'msg' => '', 'debugMsg' => '', 'data' => { 'fills' => fills } }
    end
  end

  # honeymaker's trade/fills path does not exist on BingX, so no trade was ever read. myTrades answers
  # 500 at most; 1200 trades, 300 of them stamped on the same millisecond, must all arrive once.
  test 'get_ledger reads every trade of a busy pair once' do
    start = 3.days.ago.to_i * 1000
    times = (0...900).map { |i| start + (i * 60_000) } + ([start + 450_000] * 300)
    asked = fake_ledger_client({ TRADES => trades_route(times.sort), DEPOSITS => ->(_) { [] }, WITHDRAWALS => ->(_) { [] } })

    rows = @exchange.get_ledger(api_key: ledger_api_key, start_time: 4.days.ago).data.select { |r| r[:entry_type] == :buy }

    assert_equal 1200, rows.size
    assert_equal 1200, rows.map { |r| r[:tx_id] }.uniq.size
    assert_equal ['BTC', 'USDT', 0.0000465.to_d], rows.first.values_at(:base_currency, :quote_currency, :fee_amount)
    assert(asked.select { |path, _| path == TRADES }.all? { |_, p| p[:symbol] == 'BTC-USDT' && p[:limit] == 500 })
  end

  # Beyond the past 7 days myTrades silently answers the past 24 hours instead.
  test 'get_ledger asks for trades no further back than BingX serves them' do
    asked = fake_ledger_client({ TRADES => trades_route([]), DEPOSITS => ->(_) { [] }, WITHDRAWALS => ->(_) { [] } })

    @exchange.get_ledger(api_key: ledger_api_key, start_time: 30.days.ago)

    trade_ask = asked.find { |path, _| path == TRADES }.last
    assert_operator trade_ask[:startTime], :>, 7.days.ago.to_i * 1000
  end

  test 'a failed trade or balance read fails the fetch; a pair with no trades does not' do
    fake_ledger_client({ TRADES => ->(_) { { 'code' => 100_410, 'msg' => 'rate limitation' } },
                         DEPOSITS => ->(_) { [] }, WITHDRAWALS => ->(_) { [] } })
    assert_predicate @exchange.get_ledger(api_key: ledger_api_key, start_time: 2.days.ago), :failure?

    fake_ledger_client({ TRADES => ->(_) { { 'code' => 100_204, 'msg' => 'Requested data not found' } },
                         DEPOSITS => ->(_) { [] }, WITHDRAWALS => ->(_) { [] } })
    assert_predicate @exchange.get_ledger(api_key: ledger_api_key, start_time: 2.days.ago), :success?

    Honeymaker.client('bingx').define_singleton_method(:get_raw_balances) do |**|
      Result::Success.new({ 'code' => 100_500, 'msg' => 'The current system is busy, please try again later' })
    end
    assert_predicate @exchange.get_ledger(api_key: ledger_api_key, start_time: 2.days.ago), :failure?
  end

  # Pairs were found only from coins held now or moved in the window, so the sale that emptied a coin
  # was never read once the balance was zero.
  test 'get_ledger reads a pair the account traded before even when the coin is sold out' do
    api_key = ledger_api_key
    create(:account_transaction, api_key: api_key, base_currency: 'PEPE', quote_currency: 'USDT')
    asked = fake_ledger_client({ DEPOSITS => ->(_) { [] }, WITHDRAWALS => ->(_) { [] } })
    Honeymaker.client('bingx').define_singleton_method(:get_symbols) do |**|
      Result::Success.new({ 'code' => 0, 'data' => { 'symbols' => [{ 'symbol' => 'BTC-USDT' }, { 'symbol' => 'PEPE-USDT' }] } })
    end

    @exchange.get_ledger(api_key: api_key, start_time: 2.days.ago)

    assert_equal %w[BTC-USDT PEPE-USDT], asked.select { |path, _| path == TRADES }.map { |_, p| p[:symbol] }.uniq
  end
end
