require 'test_helper'

class Exchanges::GeminiTest < ActiveSupport::TestCase
  setup do
    @exchange = create(:gemini_exchange)
  end

  test 'coingecko_id returns gemini' do
    assert_equal 'gemini', @exchange.coingecko_id
  end

  test 'known_errors includes insufficient_funds and invalid_key' do
    errors = @exchange.known_errors
    assert errors[:insufficient_funds].present?
    assert errors[:invalid_key].present?
    assert_kind_of Array, errors[:insufficient_funds]
    assert_kind_of Array, errors[:invalid_key]
  end

  test 'minimum_amount_logic returns base' do
    assert_equal :base, @exchange.minimum_amount_logic
  end

  test 'requires_passphrase? returns false (default)' do
    assert_not @exchange.requires_passphrase?
  end

  test 'set_client creates Honeymaker::Clients::Gemini instance' do
    api_key = stub(key: 'test_key', secret: 'test_secret')
    @exchange.set_client(api_key: api_key)
    assert_kind_of Honeymaker::Clients::Gemini, @exchange.instance_variable_get(:@client)
  end

  test 'set_client sets api_key reader' do
    api_key = stub(key: 'test_key', secret: 'test_secret')
    @exchange.set_client(api_key: api_key)
    assert_equal api_key, @exchange.api_key
  end

  test 'set_client handles nil api_key' do
    @exchange.set_client(api_key: nil)
    assert_nil @exchange.api_key
    assert_kind_of Honeymaker::Clients::Gemini, @exchange.instance_variable_get(:@client)
  end

  test 'get_api_key_validity uses cancel_order for trading keys' do
    Rails.configuration.stubs(:dry_run).returns(false)
    api_key = create(:api_key, exchange: @exchange, key_type: :trading, key: 'test_key', secret: 'test_secret')

    Honeymaker::Clients::Gemini.any_instance.stubs(:cancel_order).returns(
      Result::Success.new({ 'result' => 'error', 'reason' => 'OrderNotFound', 'message' => 'Order not found' })
    )
    Honeymaker::Clients::Gemini.any_instance.expects(:get_balances).never

    result = @exchange.get_api_key_validity(api_key: api_key)
    assert result.success?
    assert_equal true, result.data
  end

  test 'get_api_key_validity uses get_balances for withdrawal keys' do
    Rails.configuration.stubs(:dry_run).returns(false)
    api_key = create(:api_key, exchange: @exchange, key_type: :withdrawal, key: 'test_key', secret: 'test_secret')

    Honeymaker::Clients::Gemini.any_instance.stubs(:get_balances).returns(
      Result::Success.new([{ 'currency' => 'BTC', 'amount' => '0.5' }])
    )
    Honeymaker::Clients::Gemini.any_instance.expects(:cancel_order).never

    result = @exchange.get_api_key_validity(api_key: api_key)
    assert result.success?
    assert_equal true, result.data
  end

  test 'get_api_key_validity returns false for invalid trading key' do
    Rails.configuration.stubs(:dry_run).returns(false)
    api_key = create(:api_key, exchange: @exchange, key_type: :trading, key: 'bad_key', secret: 'bad_secret')

    Honeymaker::Clients::Gemini.any_instance.stubs(:cancel_order).returns(
      Result::Success.new({ 'result' => 'error', 'reason' => 'InvalidSignature', 'message' => 'InvalidSignature' })
    )

    result = @exchange.get_api_key_validity(api_key: api_key)
    assert result.success?
    assert_equal false, result.data
  end

  # == order-placement response parsing (Bug C) ==
  # Honeymaker's client#new_order returns { order_id: "<id>", raw: {...} } (symbol keys).
  # set_limit_order/set_market_order must read result.data[:order_id], not the string 'order_id'
  # (which raised KeyError on a successful order).

  test 'set_limit_order returns the order_id from the new_order result' do
    ticker = create(:ticker, exchange: @exchange)
    @exchange.set_client(api_key: nil)
    client = @exchange.instance_variable_get(:@client)
    client.define_singleton_method(:new_order) do |**_kwargs|
      Result::Success.new(order_id: '12345', raw: { 'order_id' => '12345' })
    end

    result = with_dry_run(false) do
      @exchange.send(:set_limit_order, ticker: ticker, amount: BigDecimal('1'),
                                       amount_type: :base, side: :buy, price: BigDecimal('100'))
    end

    assert_predicate result, :success?
    assert_equal '12345', result.data[:order_id]
  end

  test 'set_market_order returns the order_id from the new_order result' do
    ticker = create(:ticker, exchange: @exchange)
    @exchange.stubs(:get_ask_price).returns(Result::Success.new(BigDecimal('100')))
    @exchange.set_client(api_key: nil)
    client = @exchange.instance_variable_get(:@client)
    client.define_singleton_method(:new_order) do |**_kwargs|
      Result::Success.new(order_id: '12345', raw: { 'order_id' => '12345' })
    end

    result = with_dry_run(false) do
      @exchange.send(:set_market_order, ticker: ticker, amount: BigDecimal('1'),
                                        amount_type: :base, side: :buy)
    end

    assert_predicate result, :success?
    assert_equal '12345', result.data[:order_id]
  end

  # ---- get_candles (Gemini /v2/candles) ----
  # The app used to return a hardcoded [] despite honeymaker having a working /v2/candles endpoint.
  # Gemini serves a recent window, newest-first, in native resolutions only (1m/5m/15m/30m/1hr/6hr/1day);
  # coarser timeframes (4h/3d/1w/1M) are built from a finer native resolution.

  test 'get_candles maps and sorts the Gemini window, trimmed to the requested range' do
    ticker = create(:ticker, exchange: @exchange, base_symbol: 'btc', quote_symbol: 'usd')
    now = Time.now.utc
    # newest-first Array of [ts_ms, open, high, low, close, volume]
    window = (0...365).map { |i| [(now - i.days).to_i * 1000, 100.0 + i, 120.0 + i, 90.0 + i, 110.0 + i, 5.0] }
    seen = {}
    client = Object.new
    client.define_singleton_method(:get_candles) do |**kw|
      seen[:symbol] = kw[:symbol]
      seen[:time_frame] = kw[:time_frame]
      Result::Success.new(window)
    end
    @exchange.stubs(:client).returns(client)

    deep = @exchange.get_candles(ticker: ticker, start_at: 20.years.ago, timeframe: 1.day)
    assert_predicate deep, :success?
    assert_equal 365, deep.data.size
    assert_equal 'btcusd', seen[:symbol], 'must request the lowercase Gemini symbol'
    assert_equal '1day', seen[:time_frame]
    ts = deep.data.map { |c| c[0] }
    assert_equal ts.sort, ts, 'sorted ascending despite newest-first API order'
    assert_operator deep.data.last[0], :>=, now - 2.days
    assert_equal 100.to_d, deep.data.last[1], 'open mapped from index 1'
    assert_equal 110.to_d, deep.data.last[4], 'close mapped from index 4'

    recent = @exchange.get_candles(ticker: ticker, start_at: 30.days.ago, timeframe: 1.day)
    assert_operator recent.data.size, :<=, 31, 'a recent start trims the window'
  end

  test 'get_candles builds a coarser timeframe from a finer native resolution' do
    ticker = create(:ticker, exchange: @exchange, base_symbol: 'btc', quote_symbol: 'usd')
    now = Time.now.utc
    daily = (0...60).map { |i| [(now - i.days).to_i * 1000, 100.0, 120.0, 90.0, 110.0, 5.0] }
    fetched_tf = nil
    client = Object.new
    client.define_singleton_method(:get_candles) do |**kw|
      fetched_tf = kw[:time_frame]
      Result::Success.new(daily)
    end
    @exchange.stubs(:client).returns(client)

    result = @exchange.get_candles(ticker: ticker, start_at: 20.years.ago, timeframe: 1.week)
    assert_predicate result, :success?
    assert_equal '1day', fetched_tf, 'weekly is built from the daily native resolution'
    assert result.data.any?, 'built weekly candles must be non-empty (never [] -> RSI/MA crash)'
    assert_operator result.data.size, :<, 60, 'weekly aggregation reduces the daily count'
  end

  test 'get_candles returns a failure for an unsupported timeframe (no empty-array crash)' do
    ticker = create(:ticker, exchange: @exchange, base_symbol: 'btc', quote_symbol: 'usd')
    @exchange.expects(:client).never
    result = @exchange.get_candles(ticker: ticker, start_at: 1.day.ago, timeframe: 2.hours)
    assert_predicate result, :failure?
  end

  test 'get_candles returns a failure when a built timeframe has no source candles' do
    ticker = create(:ticker, exchange: @exchange, base_symbol: 'btc', quote_symbol: 'usd')
    client = Object.new
    client.define_singleton_method(:get_candles) { |**_kw| Result::Success.new([]) }
    @exchange.stubs(:client).returns(client)
    result = @exchange.get_candles(ticker: ticker, start_at: 20.years.ago, timeframe: 1.week)
    assert_predicate result, :failure?
  end

  # --- get_tickers_info: the catalogue is all or nothing -------------------------------------------
  #
  # Gemini has no bulk symbol-details endpoint, so the catalogue is one request per symbol. The sync
  # marks every ticker missing from the result unavailable, so a symbol that could not be read must
  # fail the whole catalogue rather than drop out of it.

  test 'get_tickers_info returns every symbol, pausing a second before each detail request' do
    sleeps = record_sleeps
    stub_catalogue(%w[btcusd ethusd])

    result = @exchange.get_tickers_info(force: true)

    assert_predicate result, :success?
    assert_equal(%w[btcusd ethusd], result.data.map { |t| t[:ticker] })
    assert_equal [1, 1], sleeps
  end

  test 'get_tickers_info retries a detail request dropped by the network' do
    sleeps = record_sleeps
    requested = stub_catalogue(%w[btcusd ethusd], details: { 'ethusd' => [no_answer, gemini_detail('ethusd')] })

    result = @exchange.get_tickers_info(force: true)

    assert_predicate result, :success?
    assert_equal 2, result.data.size
    assert_equal %w[btcusd ethusd ethusd], requested
    assert_equal [1, 1, 2], sleeps
  end

  test 'get_tickers_info retries a rate-limited or failing detail request' do
    record_sleeps
    [429, 408, 503].each do |status|
      failure = Result::Failure.new("HTTP #{status}", data: { status: status })
      requested = stub_catalogue(%w[btcusd], details: { 'btcusd' => [failure, gemini_detail('btcusd')] })

      assert_predicate @exchange.get_tickers_info(force: true), :success?, "HTTP #{status} is retried"
      assert_equal %w[btcusd btcusd], requested
    end
  end

  test 'get_tickers_info fails the whole catalogue when a symbol keeps failing' do
    sleeps = record_sleeps
    requested = stub_catalogue(%w[btcusd ethusd solusd], details: { 'ethusd' => no_answer })

    result = @exchange.get_tickers_info(force: true)

    assert_predicate result, :failure?
    assert_equal ['Gemini ethusd: end of file reached'], result.errors, 'names the symbol that failed'
    assert_equal %w[btcusd ethusd ethusd ethusd], requested, 'three attempts, then nothing after it'
    assert_equal [1, 1, 2, 4], sleeps
  end

  test 'get_tickers_info does not retry a rejection or a client error' do
    record_sleeps
    [Result::Failure.new('{"result":"error","reason":"InvalidSymbol"}', data: { status: 400 }),
     Result::Failure.new('NoMethodError: undefined method', data: { client_error: true })].each do |failure|
      requested = stub_catalogue(%w[btcusd ethusd], details: { 'ethusd' => failure })

      assert_predicate @exchange.get_tickers_info(force: true), :failure?
      assert_equal %w[btcusd ethusd], requested
    end
  end

  # With the exception chain reported, a dropped connection is still retried, and a failure no retry
  # can fix — a proxy refusing CONNECT — is not.
  test 'get_tickers_info decides on the exception chain when there is one' do
    record_sleeps
    closed = Result::Failure.new('end of file reached',
                                 data: { status: nil, error_chain: %w[Faraday::ConnectionFailed EOFError] })
    requested = stub_catalogue(%w[btcusd], details: { 'btcusd' => [closed, gemini_detail('btcusd')] })
    assert_predicate @exchange.get_tickers_info(force: true), :success?
    assert_equal %w[btcusd btcusd], requested

    refused = Result::Failure.new('407 "Proxy Authentication Required"',
                                  data: { status: nil, error_chain: %w[Faraday::ConnectionFailed Net::HTTPClientException] })
    requested = stub_catalogue(%w[btcusd], details: { 'btcusd' => refused })
    assert_predicate @exchange.get_tickers_info(force: true), :failure?
    assert_equal %w[btcusd], requested
  end

  test 'get_tickers_info retries the symbol list, and fails once it keeps failing' do
    sleeps = record_sleeps
    stub_catalogue(%w[btcusd], symbols: [no_answer, Result::Success.new(%w[btcusd])])
    assert_predicate @exchange.get_tickers_info(force: true), :success?
    assert_equal [2, 1], sleeps, 'one backoff, then the pause before the detail request'

    sleeps.clear
    stub_catalogue(%w[btcusd], symbols: no_answer)
    assert_predicate @exchange.get_tickers_info(force: true), :failure?
    assert_equal [2, 4], sleeps, 'three attempts, and no detail request after them'
  end

  test 'a failed catalogue is not cached' do
    record_sleeps
    Rails.stubs(:cache).returns(ActiveSupport::Cache::MemoryStore.new)
    stub_catalogue(%w[btcusd ethusd], details: { 'ethusd' => no_answer })
    assert_predicate @exchange.get_tickers_info, :failure?

    stub_catalogue(%w[btcusd ethusd])
    result = @exchange.get_tickers_info

    assert_predicate result, :success?
    assert_equal 2, result.data.size, 'the next read fetches again rather than serving a partial list'
  end

  test 'a symbol that cannot be read does not take its ticker out of the app' do
    record_sleeps
    usd = create(:asset, :usd)
    btc_usd = create(:ticker, exchange: @exchange, base_asset: create(:asset, :bitcoin), quote_asset: usd,
                              base: 'BTC', quote: 'USD', ticker: 'btcusd')
    eth_usd = create(:ticker, exchange: @exchange, base_asset: create(:asset, :ethereum), quote_asset: usd,
                              base: 'ETH', quote: 'USD', ticker: 'ethusd')
    stub_catalogue(%w[btcusd ethusd], details: { 'ethusd' => no_answer })
    MarketData.stubs(:configured?).returns(true)

    assert_predicate @exchange.sync_tickers_and_assets_with_external_data, :failure?
    assert btc_usd.reload.available
    assert eth_usd.reload.available, 'an unread symbol is not a delisted one'
  end

  private

  # Recorded, not slept, so the pacing and the retry backoff can be asserted as a sequence.
  def record_sleeps
    sleeps = []
    @exchange.define_singleton_method(:sleep) { |seconds| sleeps << seconds }
    sleeps
  end

  # What honeymaker returns when the request got no HTTP answer at all: a persistent connection the
  # venue had already closed.
  def no_answer
    Result::Failure.new('end of file reached', data: { status: nil })
  end

  def gemini_detail(symbol)
    Result::Success.new({ 'symbol' => symbol.upcase, 'base_currency' => symbol[0, 3], 'quote_currency' => symbol[3..],
                          'tick_size' => '0.00000001', 'quote_increment' => '0.01', 'min_order_size' => '0.00001',
                          'status' => 'open' })
  end

  # A client serving the symbol list (`symbols:`, default every symbol) and each symbol's details
  # (`details`, default a valid one). An Array is answered one element per call, anything else every
  # time. Returns the symbols whose details were requested, in order.
  def stub_catalogue(symbol_list, details: {}, symbols: Result::Success.new(symbol_list))
    answers = symbol_list.to_h { |symbol| [symbol, gemini_detail(symbol)] }.merge(details)
    requested = []
    client = Object.new
    client.define_singleton_method(:get_symbols) { symbols.is_a?(Array) ? symbols.shift : symbols }
    client.define_singleton_method(:get_symbol_details) do |symbol:|
      requested << symbol
      answer = answers.fetch(symbol)
      answer.is_a?(Array) ? answer.shift : answer
    end
    @exchange.stubs(:client).returns(client)
    requested
  end
end
