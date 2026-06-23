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

  test 'minimum_amount_logic returns base_or_quote for market orders' do
    assert_equal :base_or_quote, @exchange.minimum_amount_logic(order_type: :market_order)
  end

  test 'minimum_amount_logic returns base_and_quote_in_base for limit orders' do
    # Bitvavo limit orders accept only base amount + price (no amountQuote), so the
    # order setter must size limit orders in base — never :quote.
    assert_equal :base_and_quote_in_base, @exchange.minimum_amount_logic(order_type: :limit_order)
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

  # == set_limit_order converts a :quote amount to base ==
  # Bitvavo limit orders only accept base `amount` + `price` (no amountQuote). A
  # :quote amount must be converted to base at the adjusted limit price, otherwise
  # a "spend 20 EUR" request is shipped as "buy 20 BTC" -> errorCode 216.

  test 'set_limit_order converts a quote amount to a base amount at the limit price' do
    ticker = create(:ticker, exchange: @exchange, price_decimals: 0, base_decimals: 8)
    client = @exchange.send(:client)
    captured = {}
    client.define_singleton_method(:place_order) do |**kwargs|
      captured.merge!(kwargs)
      Result::Success.new(order_id: "#{kwargs[:market]}-abc123", raw: { 'orderId' => 'abc123', 'market' => kwargs[:market] })
    end

    result = with_dry_run(false) do
      @exchange.send(:set_limit_order, ticker: ticker, amount: BigDecimal('20'),
                                       amount_type: :quote, side: :buy, price: BigDecimal('59606'))
    end

    assert_predicate result, :success?
    adj_price = ticker.adjusted_price(price: BigDecimal('59606'))
    expected  = ticker.adjusted_amount(amount: BigDecimal('20') / adj_price, amount_type: :base)
    assert_equal expected.to_d.to_s('F'), captured[:amount]
    assert_nil captured[:amount_quote], 'limit orders must not send amountQuote'
    assert_operator(expected.to_d * adj_price, :<=, BigDecimal('20'), 'converted base must not over-reserve quote')
  end

  test 'set_limit_order returns a failure when the adjusted price is not positive' do
    ticker = create(:ticker, exchange: @exchange, price_decimals: 0)
    @exchange.send(:client).expects(:place_order).never

    result = with_dry_run(false) do
      @exchange.send(:set_limit_order, ticker: ticker, amount: BigDecimal('20'),
                                       amount_type: :quote, side: :buy, price: BigDecimal('0.4'))
    end

    assert_predicate result, :failure?
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

  # recover_order_from_trades reads @exchange.transactions to derive the get_trades start_time.
  # Transaction belongs to a bot AND an exchange; only exchange: @exchange matters here.
  # with_api_key: false — keep the factory simple; tests that need a client set it on @exchange.
  def waiting_bitvavo_tx(external_id, **attrs)
    bot = create(:dca_single_asset, :started, exchange: @exchange, with_api_key: false)
    create(:transaction, bot: bot, external_id: external_id,
                         status: :submitted, external_status: :open, **attrs)
  end

  # Bitvavo's order-not-found Result (errorCode 240). Faraday JSON-parses the error body, so
  # honeymaker surfaces the stringified Hash. Build it the way with_rescue does.
  def bitvavo_240_failure
    Result::Failure.new({ 'errorCode' => 240, 'error' => 'No active order found' }.to_s)
  end

  # == 240 not-found: fills-recovery FIRST, then a not_found signal (mirrors Kraken/Hyperliquid) ==

  test 'get_order recovers a 240 not-found order from get_trades when matching trades exist' do
    tx = waiting_bitvavo_tx('BTC-EUR-abc123', created_at: 2.hours.ago)
    @exchange.set_client
    @exchange.stubs(:dry_run?).returns(false)
    client = @exchange.send(:client)
    client.stubs(:get_order).with(market: 'BTC-EUR', order_id: 'abc123').returns(bitvavo_240_failure)

    expected_start = ((tx.reload.created_at - 1.hour).to_f * 1000).to_i # ms, buffered like HL/Kraken
    # First window/page: start bound asserted; window_end (start+24h) and trade_id_to (nil) are
    # time-dependent, so assert with matchers. Two matching trades (VWAP) + one unrelated-orderId
    # trade that MUST be excluded. page.size < limit → single page, single window (start+24h >= now).
    client.expects(:get_trades)
          .with(has_entries(market: 'BTC-EUR', start_time: expected_start,
                            limit: 1000, trade_id_to: nil))
          .returns(Result::Success.new([
                                         { 'orderId' => 'abc123', 'side' => 'buy', 'amount' => '0.0001', 'price' => '60000',
                                           'fee' => '0.01', 'feeCurrency' => 'EUR', 'id' => 't1', 'timestamp' => 1_700_000_000_000 },
                                         { 'orderId' => 'abc123', 'side' => 'buy', 'amount' => '0.0001', 'price' => '62000',
                                           'fee' => '0.01', 'feeCurrency' => 'EUR', 'id' => 't2', 'timestamp' => 1_700_000_100_000 },
                                         { 'orderId' => 'OTHER', 'side' => 'buy', 'amount' => '5.0', 'price' => '1',
                                           'fee' => '0', 'feeCurrency' => 'EUR', 'id' => 't9', 'timestamp' => 1_700_000_000_000 }
                                       ]))

    result = @exchange.get_order(order_id: 'BTC-EUR-abc123')

    assert result.success?
    data = result.data
    assert_equal 'BTC-EUR-abc123', data[:order_id]
    assert_equal :closed, data[:status]
    assert_equal :buy, data[:side]
    assert_equal :limit_order, data[:order_type]
    assert_equal BigDecimal('0.0002'), data[:amount_exec]            # only matching trades; 5.0 excluded
    assert_equal BigDecimal('12.2'), data[:quote_amount_exec]        # 0.0001*60000 + 0.0001*62000 = 6 + 6.2
    assert_equal BigDecimal('12.2') / BigDecimal('0.0002'), data[:price] # VWAP
    assert_equal [], data[:error_messages]
    assert data[:exchange_response].present?
  end

  test 'get_order recovers a matching trade beyond page 1, paging by tradeIdTo cursor' do
    # Lost-fill guard (Codex r1/r2): a busy account can have the target orderId only on a later page,
    # and the boundary trades can SHARE a millisecond timestamp. Paging by the `tradeIdTo` cursor
    # (not an `end - 1ms` ceiling) means a same-ms boundary trade is never skipped.
    waiting_bitvavo_tx('BTC-EUR-deep', created_at: 3.hours.ago)
    @exchange.set_client
    @exchange.stubs(:dry_run?).returns(false)
    client = @exchange.send(:client)
    client.stubs(:get_order).returns(bitvavo_240_failure)

    # Page 1: FULL (size == limit ⇒ keep paging). The OLDEST several trades all share one ms.
    full_page = Array.new(1000) do |i|
      ts = i < 997 ? 1_700_000_500_000 - i : 1_700_000_000_000 # last 3 share the same ms
      { 'orderId' => 'NOISE', 'side' => 'buy', 'amount' => '0.001', 'price' => '60000',
        'fee' => '0', 'feeCurrency' => 'EUR', 'id' => "n#{i}", 'timestamp' => ts }
    end
    # Page 2: a same-ms sibling of the boundary AND the real match — both must be fetched.
    second_page = [
      { 'orderId' => 'NOISE', 'side' => 'buy', 'amount' => '0.001', 'price' => '60000',
        'fee' => '0', 'feeCurrency' => 'EUR', 'id' => 'n_sibling', 'timestamp' => 1_700_000_000_000 },
      { 'orderId' => 'deep', 'side' => 'buy', 'amount' => '0.0002', 'price' => '61000',
        'fee' => '0.02', 'feeCurrency' => 'EUR', 'id' => 'd1', 'timestamp' => 1_699_999_900_000 }
    ]
    # The 2nd call must cursor by the oldest id on page 1 (trade_id_to), NOT a timestamp.
    client.expects(:get_trades).with(has_entries(trade_id_to: nil)).returns(Result::Success.new(full_page))
    client.expects(:get_trades).with(has_entries(trade_id_to: 'n999')).returns(Result::Success.new(second_page))

    result = @exchange.get_order(order_id: 'BTC-EUR-deep')

    assert result.success?
    assert_equal :closed, result.data[:status]
    assert_equal BigDecimal('0.0002'), result.data[:amount_exec] # only the matched 'deep' trade
  end

  test 'get_order chunks the lookback into <=24h windows for an order older than a day' do
    # Bitvavo /v2/trades caps a request window at 24h (Codex r2). A 50h-old order must be searched
    # across multiple <=24h windows, not one over-wide (and rejected/truncated) call.
    waiting_bitvavo_tx('BTC-EUR-old', created_at: 50.hours.ago)
    @exchange.set_client
    @exchange.stubs(:dry_run?).returns(false)
    client = @exchange.send(:client)
    client.stubs(:get_order).returns(bitvavo_240_failure)

    # Three windows expected: [t0, t0+24h], [t0+24h, t0+48h], [t0+48h, now]. The match lives in the
    # third. Each window returns a single short page. Assert >= 2 windows by capturing distinct
    # (start_time, end_time) pairs; the match in the last proves the chunk loop reaches it.
    windows = []
    client.stubs(:get_trades).with do |**kw|
      windows << [kw[:start_time], kw[:end_time]]
      true
    end.returns(
      Result::Success.new([]),                       # window 1: nothing
      Result::Success.new([]),                       # window 2: nothing
      Result::Success.new([                          # window 3: the fill
                            { 'orderId' => 'old', 'side' => 'buy', 'amount' => '0.0003', 'price' => '60000',
                              'fee' => '0.03', 'feeCurrency' => 'EUR', 'id' => 'o1', 'timestamp' => (Time.current.to_f * 1000).to_i - 60_000 }
                          ])
    )

    result = @exchange.get_order(order_id: 'BTC-EUR-old')

    assert result.success?
    assert_equal :closed, result.data[:status]
    assert_equal BigDecimal('0.0003'), result.data[:amount_exec]
    assert windows.size >= 2, "expected multiple <=24h windows, got #{windows.size}"
    windows.each { |s, e| assert (e - s) <= 24.hours.to_i * 1000, "window #{[s, e]} exceeds 24h" }
  end

  test 'get_order returns not_found only after exhausting all trade pages with no match' do
    waiting_bitvavo_tx('BTC-EUR-nomatch', created_at: 3.hours.ago)
    @exchange.set_client
    @exchange.stubs(:dry_run?).returns(false)
    client = @exchange.send(:client)
    client.stubs(:get_order).returns(bitvavo_240_failure)
    # One full page of unrelated trades, then a short page (also unrelated) → loop ends, no match.
    full_page = Array.new(1000) do |i|
      { 'orderId' => 'NOISE', 'side' => 'buy', 'amount' => '0.001', 'price' => '60000',
        'fee' => '0', 'feeCurrency' => 'EUR', 'id' => "n#{i}", 'timestamp' => 1_700_000_500_000 - i }
    end
    short_page = [{ 'orderId' => 'STILL-NOISE', 'side' => 'buy', 'amount' => '0.001', 'price' => '60000',
                    'fee' => '0', 'feeCurrency' => 'EUR', 'id' => 'z', 'timestamp' => 1_700_000_000_000 }]
    client.stubs(:get_trades).returns(Result::Success.new(full_page), Result::Success.new(short_page))

    result = @exchange.get_order(order_id: 'BTC-EUR-nomatch')

    assert result.failure?
    assert_equal true, result.data[:not_found]
  end

  test 'get_order propagates a page-2 failure during pagination (does not abandon)' do
    waiting_bitvavo_tx('BTC-EUR-pageerr', created_at: 3.hours.ago)
    @exchange.set_client
    @exchange.stubs(:dry_run?).returns(false)
    client = @exchange.send(:client)
    client.stubs(:get_order).returns(bitvavo_240_failure)
    full_page = Array.new(1000) do |i|
      { 'orderId' => 'NOISE', 'side' => 'buy', 'amount' => '0.001', 'price' => '60000',
        'fee' => '0', 'feeCurrency' => 'EUR', 'id' => "n#{i}", 'timestamp' => 1_700_000_500_000 - i }
    end
    client.stubs(:get_trades).returns(Result::Success.new(full_page), Result::Failure.new('Net::ReadTimeout'))

    result = @exchange.get_order(order_id: 'BTC-EUR-pageerr')

    assert result.failure?
    refute_equal true, result.data.is_a?(Hash) && result.data[:not_found] # must retry, not abandon
    assert_includes result.errors, 'Net::ReadTimeout'
  end

  test 'get_order returns a not_found signal when 240 and no matching trade exists' do
    waiting_bitvavo_tx('BTC-EUR-gone', created_at: 2.hours.ago)
    @exchange.set_client
    @exchange.stubs(:dry_run?).returns(false)
    client = @exchange.send(:client)
    client.stubs(:get_order).returns(bitvavo_240_failure)
    client.stubs(:get_trades).returns(Result::Success.new([])) # no trades at all

    result = @exchange.get_order(order_id: 'BTC-EUR-gone')

    assert result.failure?
    assert_equal true, result.data[:not_found]
    assert_includes result.data[:missing_ids], 'BTC-EUR-gone'
  end

  test 'get_order returns not_found (without calling get_trades) when 240 and no transaction row exists' do
    @exchange.set_client
    @exchange.stubs(:dry_run?).returns(false)
    client = @exchange.send(:client)
    client.stubs(:get_order).returns(bitvavo_240_failure)
    client.expects(:get_trades).never # no row → no start bound → bail straight to not_found

    result = @exchange.get_order(order_id: 'BTC-EUR-orphan')

    assert result.failure?
    assert_equal true, result.data[:not_found]
  end

  test 'get_order propagates a transient get_trades failure so the job can retry' do
    waiting_bitvavo_tx('BTC-EUR-txerr', created_at: 2.hours.ago)
    @exchange.set_client
    @exchange.stubs(:dry_run?).returns(false)
    client = @exchange.send(:client)
    client.stubs(:get_order).returns(bitvavo_240_failure)
    client.stubs(:get_trades).returns(Result::Failure.new('Net::ReadTimeout'))

    result = @exchange.get_order(order_id: 'BTC-EUR-txerr')

    assert result.failure?
    refute_equal true, result.data.is_a?(Hash) && result.data[:not_found] # NOT abandoned — must retry
    assert_includes result.errors, 'Net::ReadTimeout'
  end

  test 'get_order propagates a non-240 failure unchanged (no fills recovery)' do
    @exchange.set_client
    @exchange.stubs(:dry_run?).returns(false)
    client = @exchange.send(:client)
    client.stubs(:get_order).returns(Result::Failure.new('Invalid API key.', data: { status: 401 }))
    client.expects(:get_trades).never # not a 240 → do not treat as order-gone

    result = @exchange.get_order(order_id: 'BTC-EUR-xyz')

    assert result.failure?
    assert_includes result.errors, 'Invalid API key.'
    refute_equal true, result.data.is_a?(Hash) && result.data[:not_found]
  end

  # == get_orders: collect not-found ids under :missing (Kraken/HL bulk contract) ==

  test 'get_orders lists a 240 not-found id under :missing and returns the rest' do
    create(:ticker, exchange: @exchange, ticker: 'BTC-EUR', base: 'BTC', quote: 'EUR')
    @exchange.set_client
    @exchange.stubs(:dry_run?).returns(false)
    client = @exchange.send(:client)
    found_raw = { 'market' => 'BTC-EUR', 'orderId' => 'found1', 'orderType' => 'limit',
                  'side' => 'buy', 'status' => 'filled', 'price' => '60000',
                  'filledAmount' => '0.0001', 'filledAmountQuote' => '6.0' }
    client.stubs(:get_order).with(market: 'BTC-EUR', order_id: 'found1')
          .returns(Result::Success.new(raw: found_raw))
    # No transaction row for the gone id → recovery bails straight to not_found.
    client.stubs(:get_order).with(market: 'BTC-EUR', order_id: 'gone1').returns(bitvavo_240_failure)

    result = @exchange.get_orders(order_ids: %w[BTC-EUR-found1 BTC-EUR-gone1])

    assert result.success?
    assert_equal %w[BTC-EUR-found1], result.data[:orders].keys
    assert_equal %w[BTC-EUR-gone1], result.data[:missing]
  end

  test 'get_orders propagates a non-not_found failure instead of swallowing it into :missing' do
    @exchange.set_client
    @exchange.stubs(:dry_run?).returns(false)
    client = @exchange.send(:client)
    # A transient/timeout failure (no 240, no not_found) must abort the batch so the job retries,
    # NOT be silently degraded to a "missing" id (which would mis-trigger abandonment).
    client.stubs(:get_order).returns(Result::Failure.new('Net::ReadTimeout'))

    result = @exchange.get_orders(order_ids: %w[BTC-EUR-x])

    assert result.failure?
    assert_includes result.errors, 'Net::ReadTimeout'
  end

  test 'get_orders recovers a 240 order from get_trades in the bulk path' do
    waiting_bitvavo_tx('BTC-EUR-r1', created_at: 2.hours.ago)
    @exchange.set_client
    @exchange.stubs(:dry_run?).returns(false)
    client = @exchange.send(:client)
    client.stubs(:get_order).with(market: 'BTC-EUR', order_id: 'r1').returns(bitvavo_240_failure)
    trades = [{ 'orderId' => 'r1', 'side' => 'buy', 'amount' => '0.0001', 'price' => '60000',
                'fee' => '0.01', 'feeCurrency' => 'EUR', 'id' => 't1', 'timestamp' => 1_700_000_000_000 }]
    client.stubs(:get_trades).returns(Result::Success.new(trades))

    result = @exchange.get_orders(order_ids: %w[BTC-EUR-r1])

    assert result.success?
    assert_empty result.data[:missing]
    assert_equal :closed, result.data[:orders]['BTC-EUR-r1'][:status]
  end

  # == authoritative: get_trades makes a still-missing order confirmed never-executed ==

  test 'authoritative_missing_orders? is true' do
    assert @exchange.authoritative_missing_orders?
  end
end
