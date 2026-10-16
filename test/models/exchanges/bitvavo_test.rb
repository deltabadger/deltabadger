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
    # Matched against Bitvavo's captured prose (errorCode 216 / 305), not the strings this venue
    # was previously configured with — none of which appear in any real response.
    assert_includes errors[:insufficient_funds], 'sufficient balance to complete'
    assert_includes errors[:invalid_key], 'No active API key found'
  end

  # Bodies as honeymaker hands them back (the raw JSON of the HTTP error), codes and reasons from
  # https://docs.bitvavo.com/docs/errors/ and ccxt's captured messages.
  def bitvavo_error(code, message)
    { errorCode: code, error: message }.to_json
  end

  test 'the documented key, permission, rate-limit and outage errors are classified' do
    assert @exchange.invalid_key_error?([bitvavo_error(309, 'The signature is invalid.')])
    assert @exchange.invalid_key_error?([bitvavo_error(308, 'The signature length is invalid (HMAC-SHA256 should return a 64 length hexadecimal string).')])

    %w[307 310 311].each do |code|
      body = bitvavo_error(code.to_i, 'This key does not allow this action.')
      assert @exchange.permission_error?([body]), code
      assert_not @exchange.invalid_key_error?([body]), code
    end

    assert @exchange.throttled_error?([bitvavo_error(105, 'Your IP or API key has been banned for not respecting the rate limit. The ban expires at 1792139914630.')])
    [101, 107, 108, 109, 111, 400, 419].each do |code|
      assert @exchange.transient_error?([bitvavo_error(code, 'Bitvavo is overloaded.')]), code.to_s
    end
    # A request outside the access window is refused before it reaches the matching engine.
    stale = bitvavo_error(304, 'Request was not received within acceptable window (default 30s, or custom with Bitvavo-Access-Window header) of Bitvavo-Access-Timestamp header.')
    assert @exchange.transient_error?([stale])
    assert @exchange.placement_transient_error?([stale])
    assert_not @exchange.placement_transient_error?([bitvavo_error(109, 'The matching engine did not respond in time.')])
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

    # The shape honeymaker hands back for Bitvavo's HTTP 404: the raw body, and the status.
    Honeymaker::Clients::Bitvavo.any_instance.stubs(:cancel_order).returns(
      Result::Failure.new({ errorCode: 240, error: 'No order found.' }.to_json, data: { status: 404 })
    )
    Honeymaker::Clients::Bitvavo.any_instance.stubs(:get_raw_balance).returns(Result::Success.new([]))

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
    Honeymaker::Clients::Bitvavo.any_instance.stubs(:get_raw_balance).returns(Result::Success.new([]))

    # Bitvavo's own words for a dead key (ERRORS[:invalid_key]); a bare 401 could be our proxy.
    Honeymaker::Clients::Bitvavo.any_instance.stubs(:cancel_order).returns(
      Result::Failure.new({ errorCode: 305, error: 'No active API key found.' }.to_json, data: { status: 403 })
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

  # https://docs.bitvavo.com/docs/rest-api/get-candlestick-data/: newest first, at most 1440, and a
  # range longer than that returns its LATEST 1440 (checked live on BTC-EUR 1d from 2017: the oldest
  # row came back as 2022-11-07). `end` is exclusive of the candle that starts at it.
  test 'get_candles pages back through a range longer than one page and returns it oldest first' do
    ticker = create(:ticker, exchange: @exchange, ticker: 'BTC-EUR', base: 'BTC', quote: 'EUR')
    hour = 3_600_000
    newest = 1_792_137_600_000
    candle = ->(ts) { [ts, '73150', '73194', '73000', '73124', '6.5'] }
    latest = Array.new(1440) { |i| candle.call(newest - (i * hour)) }
    oldest_on_page = newest - (1439 * hour)
    older = [candle.call(oldest_on_page - hour), candle.call(oldest_on_page - (2 * hour))]
    start_at = Time.at((oldest_on_page - (2 * hour)) / 1000).utc
    client = @exchange.send(:client)
    client.expects(:get_candles).with(has_entries(market: 'BTC-EUR', interval: '1h', start_time: start_at.to_i * 1000, end_time: nil))
          .returns(Result::Success.new(latest))
    client.expects(:get_candles).with(has_entries(start_time: start_at.to_i * 1000, end_time: oldest_on_page))
          .returns(Result::Success.new(older))

    candles = @exchange.get_candles(ticker: ticker, start_at: start_at, timeframe: 1.hour).data

    assert_equal 1442, candles.size
    assert_equal start_at, candles.first[0]
    assert_equal Time.at(newest / 1000).utc, candles.last[0]
    assert_equal candles.map(&:first).sort, candles.map(&:first)
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
    client_class.any_instance.stubs(:get_signed).with('/v2/depositHistory', anything).returns(Result::Success.new([]))
    client_class.any_instance.stubs(:get_signed).with('/v2/withdrawalHistory', anything).returns(Result::Success.new([]))

    result = nil
    assert_nothing_raised { result = @exchange.get_ledger(api_key: api_key) }
    assert_predicate result, :success?
    assert(result.data.any? { |e| e[:entry_type] == :buy && e[:base_currency] == 'BTC' })
  end

  # Shapes from https://docs.bitvavo.com/docs/rest-api/get-deposit-history/ and
  # …/get-withdrawal-history/ (status on a crypto deposit as ccxt captured it).
  def bitvavo_deposit(timestamp, amount, **extra)
    { 'timestamp' => timestamp, 'symbol' => 'BTC', 'amount' => amount, 'address' => '14qViLJfdGaP4EeHnDyJbEGQysnCpwk3gd',
      'txId' => "927b3ea50c5bb52c6854152d305dfa1e27fc01d10464cf10825d96d69d235eb3\n", 'fee' => '0' }.merge(extra)
  end

  def stub_empty_trades(client_class)
    client_class.any_instance.stubs(:get_raw_balance).returns(Result::Success.new([]))
    client_class.any_instance.stubs(:get_markets).returns(Result::Success.new([]))
  end

  test 'get_ledger reads every deposit page from depositHistory, re-reading the boundary millisecond' do
    api_key = create(:api_key, exchange: @exchange, key_type: :trading, key: 'k', secret: 's')
    client_class = Honeymaker::Clients::Bitvavo
    stub_empty_trades(client_class)
    boundary = 1_700_000_000_000
    # A full page, newest first; its two oldest rows share one millisecond.
    full = Array.new(1000) { |i| bitvavo_deposit(boundary + 1000 - i, "0.#{i + 1}") }
    full[-1] = bitvavo_deposit(boundary, '0.5')
    full[-2] = bitvavo_deposit(boundary, '0.25')
    # The next page re-reads that millisecond: a repeat, a sibling the first page cut off, and older rows.
    rest = [bitvavo_deposit(boundary, '0.5'), bitvavo_deposit(boundary, '0.125'),
            bitvavo_deposit(boundary - 5, '10', 'symbol' => 'EUR', 'status' => 'completed', 'txId' => nil),
            bitvavo_deposit(boundary - 9, '1', 'status' => 'canceled')]
    client_class.any_instance.expects(:get_signed)
                .with('/v2/depositHistory', has_entries(start: 0, limit: 1000)).returns(Result::Success.new(full))
    client_class.any_instance.expects(:get_signed)
                .with('/v2/depositHistory', has_entries(start: 0, end: boundary + 1)).returns(Result::Success.new(rest))
    client_class.any_instance.stubs(:get_signed).with('/v2/withdrawalHistory', anything).returns(
      Result::Success.new([{ 'timestamp' => boundary, 'symbol' => 'BTC', 'amount' => '0.3', 'fee' => '0.0001',
                             'status' => 'completed', 'txId' => 'abc' },
                           { 'timestamp' => boundary, 'symbol' => 'BTC', 'amount' => '0.3', 'fee' => '0.0001',
                             'status' => 'awaiting_processing' }])
    )

    result = @exchange.get_ledger(api_key: api_key)

    assert result.success?, result.errors.inspect
    deposits = result.data.select { |e| e[:entry_type] == :deposit }
    assert_equal 1002, deposits.size # 1000 + the cut-off sibling + the fiat deposit; repeat and cancelled dropped
    assert(deposits.any? { |e| e[:base_amount] == BigDecimal('0.125') })
    assert(deposits.any? { |e| e[:base_currency] == 'EUR' })
    assert(deposits.all? { |e| e[:tx_id].nil? }, 'a chain hash is not a record id')
    withdrawals = result.data.select { |e| e[:entry_type] == :withdrawal }
    assert_equal 1, withdrawals.size
    assert_nil withdrawals.first[:tx_id]
  end

  test 'get_ledger fails when a transfer read fails' do
    api_key = create(:api_key, exchange: @exchange, key_type: :trading, key: 'k', secret: 's')
    client_class = Honeymaker::Clients::Bitvavo
    stub_empty_trades(client_class)
    client_class.any_instance.stubs(:get_signed).with('/v2/depositHistory', anything).returns(Result::Success.new([]))
    client_class.any_instance.stubs(:get_signed).with('/v2/withdrawalHistory', anything).returns(
      Result::Failure.new({ errorCode: 311, error: 'This key does not allow showing account information.' }.to_json, data: { status: 403 })
    )

    result = @exchange.get_ledger(api_key: api_key)

    assert result.failure?
    assert @exchange.permission_error?(result.errors)
  end

  test 'get_ledger fails when a full transfer page brings nothing new' do
    api_key = create(:api_key, exchange: @exchange, key_type: :trading, key: 'k', secret: 's')
    client_class = Honeymaker::Clients::Bitvavo
    stub_empty_trades(client_class)
    same_ms = Array.new(1000) { |i| bitvavo_deposit(1_700_000_000_000, "0.#{i + 1}") }
    client_class.any_instance.stubs(:get_signed).with('/v2/depositHistory', anything).returns(Result::Success.new(same_ms))

    assert @exchange.get_ledger(api_key: api_key).failure?
  end

  # Trades for the ledger. Checked live on the public trades endpoint, which takes the same
  # parameters: start alone is refused, start..end must span 24 hours or less, tradeIdTo alone pages
  # back to any age.
  def ledger_with_trades(markets: %w[BTC-EUR], balances: [{ 'symbol' => 'BTC', 'available' => '0.5', 'inOrder' => '0' }], &trades)
    api_key = create(:api_key, exchange: @exchange, key_type: :trading, key: 'k', secret: 's')
    client_class = Honeymaker::Clients::Bitvavo
    client_class.any_instance.stubs(:get_signed).with(regexp_matches(/History\z/), anything).returns(Result::Success.new([]))
    client_class.any_instance.stubs(:get_raw_balance).returns(Result::Success.new(balances))
    client_class.any_instance.stubs(:get_markets).returns(
      Result::Success.new(markets.map { |m| { 'market' => m, 'base' => m.split('-').first, 'quote' => m.split('-').last } })
    )
    asked = []
    client_class.any_instance.stubs(:get_trades).with do |**params|
      asked << params
      true
    end.returns(Result::Failure.new('stub me'))
    yield client_class, api_key if trades
    [api_key, asked]
  end

  test 'get_ledger pages back through every trade since the last sync without start, by trade id' do
    start_205 = Result::Failure.new({ errorCode: 205, error: 'end parameter is invalid. start and end must be provided together.' }.to_json,
                                    data: { status: 400 })
    top = ms_ago(1.minute)
    full = Array.new(1000) { |i| bitvavo_trade("n#{i}", 'o', top - i) }
    older = [bitvavo_trade('n1000', 'o', ms_ago(3.hours)), bitvavo_trade('n1001', 'o', ms_ago(3.days))]
    api_key, = ledger_with_trades do |client_class|
      client_class.any_instance.stubs(:get_trades).with(has_key(:start_time)).returns(start_205)
      client_class.any_instance.stubs(:get_trades).with(Not(has_key(:start_time))).returns(Result::Success.new(full), Result::Success.new(older))
    end

    result = @exchange.get_ledger(api_key: api_key, start_time: 1.day.ago)

    assert result.success?, result.errors.inspect
    ids = result.data.map { |e| e[:tx_id] }
    assert_equal 1001, ids.size # the 3-day-old trade is before the window
    assert_includes ids, 'n1000'
    trade = result.data.find { |e| e[:tx_id] == 'n0' }
    assert_equal :buy, trade[:entry_type]
    assert_equal %w[BTC EUR], [trade[:base_currency], trade[:quote_currency]]
    assert_equal BigDecimal('60'), trade[:quote_amount]
  end

  test 'get_ledger fails when a trade read fails' do
    api_key, = ledger_with_trades do |client_class|
      client_class.any_instance.stubs(:get_trades).returns(
        Result::Failure.new({ errorCode: 310, error: 'This key does not allow trading actions.' }.to_json, data: { status: 403 })
      )
    end

    result = @exchange.get_ledger(api_key: api_key)

    assert result.failure?
    assert @exchange.permission_error?(result.errors)
  end

  test 'get_ledger fails when the balance read fails' do
    api_key, = ledger_with_trades
    Honeymaker::Clients::Bitvavo.any_instance.stubs(:get_raw_balance).returns(Result::Failure.new('Net::ReadTimeout'))

    assert @exchange.get_ledger(api_key: api_key).failure?
  end

  test 'get_ledger reads a market the account traded before, or a bot ordered on, after the coin is sold out' do
    api_key, asked = ledger_with_trades(markets: %w[BTC-EUR ETH-EUR SOL-EUR ADA-EUR], balances: [])
    Honeymaker::Clients::Bitvavo.any_instance.stubs(:get_trades).with do |**params|
      asked << params
      true
    end.returns(Result::Success.new([]))
    create(:account_transaction, api_key: api_key, base_currency: 'ETH', quote_currency: 'EUR')
    bot = create(:dca_single_asset, user: api_key.user, exchange: @exchange)
    create(:transaction, bot: bot, use_bot_assets: false, resolve_asset_ids: false, base: 'SOL', quote: 'EUR')

    assert @exchange.get_ledger(api_key: api_key).success?
    assert_equal %w[ETH-EUR SOL-EUR], asked.map { |p| p[:market] }.uniq.sort
  end

  # recover_order_from_trades reads @exchange.transactions to derive the get_trades start_time.
  # Transaction belongs to a bot AND an exchange; only exchange: @exchange matters here.
  # with_api_key: false — keep the factory simple; tests that need a client set it on @exchange.
  def waiting_bitvavo_tx(external_id, **attrs)
    bot = create(:dca_single_asset, :started, exchange: @exchange, with_api_key: false)
    create(:transaction, bot: bot, external_id: external_id,
                         status: :submitted, external_status: :open, **attrs)
  end

  # Bitvavo's order-not-found Result: HTTP 404 with errorCode 240, the raw JSON body as honeymaker's
  # with_rescue hands it back. Wording as ccxt captured it; the docs give the code only.
  def bitvavo_240_failure
    Result::Failure.new(
      { errorCode: 240, error: 'No order found. Please be aware that simultaneously updating the same order may return this error.' }.to_json,
      data: { status: 404 }
    )
  end

  test 'get_order does not read a 404 without Bitvavo\'s errorCode 240 as a gone order' do
    @exchange.set_client
    @exchange.stubs(:dry_run?).returns(false)
    client = @exchange.send(:client)
    client.stubs(:get_order).returns(Result::Failure.new('<html><body><h1>Resource not found</h1></body></html>', data: { status: 404 }))
    client.expects(:get_trades).never

    result = @exchange.get_order(order_id: 'BTC-EUR-abc')

    assert result.failure?
    refute_equal true, result.data[:not_found]
  end

  # == 240 not-found: fills-recovery FIRST, then a not_found signal (mirrors Kraken/Hyperliquid) ==

  def ms_ago(duration)
    (duration.ago.to_f * 1000).to_i
  end

  def bitvavo_trade(id, order_id, timestamp, amount: '0.001', price: '60000')
    { 'id' => id, 'orderId' => order_id, 'timestamp' => timestamp, 'market' => 'BTC-EUR', 'side' => 'buy',
      'amount' => amount, 'price' => price, 'taker' => true, 'fee' => '0.01', 'feeCurrency' => 'EUR', 'settled' => true }
  end

  test 'get_order recovers a 240 not-found order from get_trades when matching trades exist' do
    waiting_bitvavo_tx('BTC-EUR-abc123', created_at: 2.hours.ago)
    @exchange.set_client
    @exchange.stubs(:dry_run?).returns(false)
    client = @exchange.send(:client)
    client.stubs(:get_order).with(market: 'BTC-EUR', order_id: 'abc123').returns(bitvavo_240_failure)

    # Two matching trades (VWAP) + one unrelated-orderId trade that MUST be excluded. A short page
    # ends the walk.
    client.expects(:get_trades)
          .with(has_entries(market: 'BTC-EUR', limit: 1000, trade_id_to: nil))
          .returns(Result::Success.new([
                                         bitvavo_trade('t9', 'OTHER', ms_ago(30.minutes), amount: '5.0', price: '1'),
                                         bitvavo_trade('t2', 'abc123', ms_ago(59.minutes), amount: '0.0001', price: '62000'),
                                         bitvavo_trade('t1', 'abc123', ms_ago(1.hour), amount: '0.0001', price: '60000')
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
    # A busy account can have the target orderId only on a later page, and the boundary trades can
    # SHARE a millisecond. tradeIdTo continues strictly before the page's oldest id, so a same-ms
    # boundary trade is never skipped.
    waiting_bitvavo_tx('BTC-EUR-deep', created_at: 3.hours.ago)
    @exchange.set_client
    @exchange.stubs(:dry_run?).returns(false)
    client = @exchange.send(:client)
    client.stubs(:get_order).returns(bitvavo_240_failure)

    top = ms_ago(1.minute)
    full_page = Array.new(1000) { |i| bitvavo_trade("n#{i}", 'NOISE', i < 997 ? top - i : top - 5000) }
    second_page = [bitvavo_trade('n_sibling', 'NOISE', top - 5000),
                   bitvavo_trade('d1', 'deep', ms_ago(2.hours), amount: '0.0002', price: '61000')]
    client.expects(:get_trades).with(has_entries(trade_id_to: nil)).returns(Result::Success.new(full_page))
    client.expects(:get_trades).with(has_entries(trade_id_to: 'n999')).returns(Result::Success.new(second_page))

    result = @exchange.get_order(order_id: 'BTC-EUR-deep')

    assert result.success?
    assert_equal :closed, result.data[:status]
    assert_equal BigDecimal('0.0002'), result.data[:amount_exec] # only the matched 'deep' trade
  end

  test 'get_order finds the fill of an order resting for longer than 40 days' do
    # The lookback used to walk at most 40 one-day windows from the order's row, so a fill after that
    # was never searched and the order read as never executed.
    waiting_bitvavo_tx('BTC-EUR-old', created_at: 50.days.ago)
    @exchange.set_client
    @exchange.stubs(:dry_run?).returns(false)
    client = @exchange.send(:client)
    client.stubs(:get_order).returns(bitvavo_240_failure)
    client.expects(:get_trades).with(Not(has_key(:start_time))).returns(
      Result::Success.new([bitvavo_trade('o1', 'old', ms_ago(1.minute), amount: '0.0003')])
    )

    result = @exchange.get_order(order_id: 'BTC-EUR-old')

    assert result.success?
    assert_equal :closed, result.data[:status]
    assert_equal BigDecimal('0.0003'), result.data[:amount_exec]
  end

  test 'get_order returns not_found only after exhausting all trade pages with no match' do
    waiting_bitvavo_tx('BTC-EUR-nomatch', created_at: 3.hours.ago)
    @exchange.set_client
    @exchange.stubs(:dry_run?).returns(false)
    client = @exchange.send(:client)
    client.stubs(:get_order).returns(bitvavo_240_failure)
    full_page = Array.new(1000) { |i| bitvavo_trade("n#{i}", 'NOISE', ms_ago(1.minute) - i) }
    short_page = [bitvavo_trade('z', 'STILL-NOISE', ms_ago(2.hours))]
    client.expects(:get_trades).twice.returns(Result::Success.new(full_page), Result::Success.new(short_page))

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
    full_page = Array.new(1000) { |i| bitvavo_trade("n#{i}", 'NOISE', ms_ago(1.minute) - i) }
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

  # https://docs.bitvavo.com/docs/rest-api/get-order/: `price` is the limit set on the order,
  # filledAmount / filledAmountQuote are what traded.
  test 'get_order prices a filled limit order at its fills, not at its limit' do
    create(:ticker, exchange: @exchange, ticker: 'BTC-EUR', base: 'BTC', quote: 'EUR', price_decimals: 0)
    @exchange.set_client
    @exchange.stubs(:dry_run?).returns(false)
    raw = { 'market' => 'BTC-EUR', 'orderId' => 'lim1', 'orderType' => 'limit', 'side' => 'buy',
            'status' => 'filled', 'price' => '60000', 'amount' => '0.002', 'amountRemaining' => '0',
            'filledAmount' => '0.002', 'filledAmountQuote' => '118', 'feePaid' => '0.29', 'feeCurrency' => 'EUR' }
    @exchange.send(:client).stubs(:get_order).returns(Result::Success.new(raw: raw))

    result = @exchange.get_order(order_id: 'BTC-EUR-lim1')

    assert result.success?
    assert_equal BigDecimal('59000'), result.data[:price]
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
    trades = [bitvavo_trade('t1', 'r1', ms_ago(1.hour), amount: '0.0001')]
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
