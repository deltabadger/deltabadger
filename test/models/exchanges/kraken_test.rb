require 'test_helper'

class Exchanges::KrakenTest < ActiveSupport::TestCase
  setup do
    @exchange = create(:kraken_exchange)
    Rails.configuration.stubs(:dry_run).returns(false)
  end

  def kraken_pairs(status: :none)
    info = {
      'altname' => 'XBTUSDT', 'wsname' => 'XBT/USDT', 'ordermin' => '0.0001',
      'costmin' => '0.5', 'lot_decimals' => 8, 'cost_decimals' => 5, 'pair_decimals' => 1
    }
    info['status'] = status unless status == :none
    { 'error' => [], 'result' => { 'XBTUSDT' => info } }
  end

  # A waiting Kraken transaction owned by @exchange (recover_missing_from_trades reads
  # @exchange.transactions). The :started bot provisions the ticker/assets on @exchange so
  # the transaction's broadcast_new_order callback can resolve decimals.
  def waiting_kraken_tx(external_id, **attrs)
    bot = create(:dca_single_asset, :started, exchange: @exchange)
    create(:transaction, bot: bot, external_id: external_id,
                         status: :submitted, external_status: :open, **attrs)
  end

  test 'get_tickers_info marks an online pair available and trading_enabled' do
    @exchange.set_client
    @exchange.send(:client).stubs(:get_tradable_asset_pairs).returns(Result::Success.new(kraken_pairs(status: 'online')))

    ticker = @exchange.get_tickers_info(force: true).data.first
    assert ticker[:available]
    assert ticker[:trading_enabled]
  end

  test 'get_tickers_info marks a non-online pair listed but not trading_enabled' do
    @exchange.set_client
    @exchange.send(:client).stubs(:get_tradable_asset_pairs).returns(Result::Success.new(kraken_pairs(status: 'cancel_only')))

    ticker = @exchange.get_tickers_info(force: true).data.first
    assert ticker[:available]
    assert_not ticker[:trading_enabled]
  end

  test 'get_tickers_info defaults trading_enabled to true when status is absent' do
    @exchange.set_client
    @exchange.send(:client).stubs(:get_tradable_asset_pairs).returns(Result::Success.new(kraken_pairs(status: :none)))

    assert @exchange.get_tickers_info(force: true).data.first[:trading_enabled]
  end

  test 'get_api_key_validity uses add_order for trading keys' do
    api_key = create(:api_key, exchange: @exchange, key_type: :trading, key: 'test_key', secret: 'dGVzdF9zZWNyZXQ=')

    Honeymaker::Clients::Kraken.any_instance.stubs(:add_order).returns(
      Result::Success.new({ 'error' => [] })
    )
    Honeymaker::Clients::Kraken.any_instance.expects(:get_extended_balance).never

    result = @exchange.get_api_key_validity(api_key: api_key)
    assert result.success?
    assert_equal true, result.data
  end

  test 'get_api_key_validity uses get_extended_balance for withdrawal keys' do
    api_key = create(:api_key, exchange: @exchange, key_type: :withdrawal, key: 'test_key', secret: 'dGVzdF9zZWNyZXQ=')

    Honeymaker::Clients::Kraken.any_instance.stubs(:get_extended_balance).returns(
      Result::Success.new({ 'error' => [], 'result' => {} })
    )
    Honeymaker::Clients::Kraken.any_instance.expects(:add_order).never

    result = @exchange.get_api_key_validity(api_key: api_key)
    assert result.success?
    assert_equal true, result.data
  end

  test 'get_api_key_validity returns incorrect for invalid withdrawal key' do
    api_key = create(:api_key, exchange: @exchange, key_type: :withdrawal, key: 'bad_key', secret: 'dGVzdF9zZWNyZXQ=')

    Honeymaker::Clients::Kraken.any_instance.stubs(:get_extended_balance).returns(
      Result::Success.new({ 'error' => ['EAPI:Invalid key'] })
    )

    result = @exchange.get_api_key_validity(api_key: api_key)
    assert result.success?
    assert_equal false, result.data
  end

  test 'get_api_key_validity returns incorrect for permission denied on withdrawal key' do
    api_key = create(:api_key, exchange: @exchange, key_type: :withdrawal, key: 'bad_key', secret: 'dGVzdF9zZWNyZXQ=')

    Honeymaker::Clients::Kraken.any_instance.stubs(:get_extended_balance).returns(
      Result::Success.new({ 'error' => ['EGeneral:Permission denied'] })
    )

    result = @exchange.get_api_key_validity(api_key: api_key)
    assert result.success?
    assert_equal false, result.data
  end

  # list_withdrawal_addresses includes key field

  test 'list_withdrawal_addresses includes key field from API response' do
    asset = create(:asset, :bitcoin)
    create(:ticker, exchange: @exchange, base_asset: asset, quote_asset: create(:asset, :usd))

    Honeymaker::Clients::Kraken.any_instance.stubs(:get_withdraw_addresses).returns(
      Result::Success.new(
        'error' => [],
        'result' => [
          { 'address' => 'bc1q...abc', 'key' => 'My BTC Wallet', 'method' => 'Bitcoin', 'verified' => true },
          { 'address' => 'bc1q...def', 'key' => 'Cold Storage', 'method' => 'Bitcoin', 'verified' => true },
          { 'address' => 'bc1q...unverified', 'key' => 'Unverified', 'method' => 'Bitcoin', 'verified' => false }
        ]
      )
    )

    addresses = @exchange.list_withdrawal_addresses(asset: asset)

    assert_equal 2, addresses.size
    assert_equal 'bc1q...abc', addresses[0][:name]
    assert_equal 'My BTC Wallet', addresses[0][:key]
    assert_equal 'bc1q...abc - My BTC Wallet - Bitcoin', addresses[0][:label]
    assert_equal 'bc1q...def', addresses[1][:name]
    assert_equal 'Cold Storage', addresses[1][:key]
  end

  # withdraw looks up key name

  test 'withdraw passes key name instead of raw address to API' do
    asset = create(:asset, :bitcoin)
    create(:ticker, exchange: @exchange, base_asset: asset, quote_asset: create(:asset, :usd))

    api_key = create(:api_key, exchange: @exchange, key_type: :withdrawal, key: 'test_key', secret: 'dGVzdF9zZWNyZXQ=')
    @exchange.set_client(api_key: api_key)

    Honeymaker::Clients::Kraken.any_instance.stubs(:get_withdraw_addresses).returns(
      Result::Success.new(
        'error' => [],
        'result' => [
          { 'address' => 'bc1q...abc', 'key' => 'My BTC Wallet', 'method' => 'Bitcoin', 'verified' => true }
        ]
      )
    )

    Honeymaker::Clients::Kraken.any_instance.expects(:withdraw).with(
      asset: 'BTC',
      key: 'My BTC Wallet',
      amount: '0.5',
      address: 'bc1q...abc'
    ).returns(Result::Success.new({ 'error' => [], 'result' => { 'refid' => 'ATEST' } }))

    result = @exchange.withdraw(asset: asset, amount: BigDecimal('0.5'), address: 'bc1q...abc')

    assert result.success?
    assert_equal 'ATEST', result.data[:withdrawal_id]
  end

  test 'withdraw falls back to address when key name not found' do
    asset = create(:asset, :bitcoin)
    create(:ticker, exchange: @exchange, base_asset: asset, quote_asset: create(:asset, :usd))

    api_key = create(:api_key, exchange: @exchange, key_type: :withdrawal, key: 'test_key', secret: 'dGVzdF9zZWNyZXQ=')
    @exchange.set_client(api_key: api_key)

    Honeymaker::Clients::Kraken.any_instance.stubs(:get_withdraw_addresses).returns(
      Result::Success.new('error' => [], 'result' => [])
    )

    Honeymaker::Clients::Kraken.any_instance.expects(:withdraw).with(
      asset: 'BTC',
      key: 'bc1q...unknown',
      amount: '0.5',
      address: 'bc1q...unknown'
    ).returns(Result::Success.new({ 'error' => [], 'result' => { 'refid' => 'BTEST' } }))

    result = @exchange.withdraw(asset: asset, amount: BigDecimal('0.5'), address: 'bc1q...unknown')

    assert result.success?
  end

  # == get_orders shape contract ==
  # Kraken silently omits IDs it no longer tracks (e.g. orders aged out of the
  # QueryOrders retention window). The new contract is { orders:, missing: }
  # so callers can act on the dropped IDs instead of polling them forever.

  test 'get_orders returns { orders:, missing: } with missing: [] when Kraken returns every requested ID' do
    base = create(:asset, :bitcoin)
    quote = create(:asset, :usd)
    create(:ticker, exchange: @exchange, base_asset: base, quote_asset: quote, ticker: 'XBTUSD')

    @exchange.set_client
    @exchange.send(:client).stubs(:query_orders_info).returns(
      Result::Success.new(
        'TXID-A' => { raw: kraken_order_raw('XBTUSD', 'closed'), status: :closed },
        'TXID-B' => { raw: kraken_order_raw('XBTUSD', 'closed'), status: :closed }
      )
    )

    result = @exchange.get_orders(order_ids: %w[TXID-A TXID-B])

    assert result.success?
    assert_kind_of Hash, result.data
    assert_equal %i[orders missing].sort, result.data.keys.sort
    assert_equal %w[TXID-A TXID-B].sort, result.data[:orders].keys.sort
    assert_equal [], result.data[:missing]
  end

  test 'get_order returns Result::Failure with not_found flag when Kraken returns an empty result for the txid' do
    @exchange.set_client
    # Kraken returned 200 OK but the requested txid is absent — the order has
    # aged out of QueryOrders retention. Surface this distinctly so callers can
    # stop polling instead of retrying forever.
    @exchange.send(:client).stubs(:query_orders_info).returns(Result::Success.new({}))
    # No fill in TradesHistory either → genuinely not_found.
    @exchange.send(:client).stubs(:closed_orders_from_trades).returns(Result::Success.new({}))

    result = @exchange.get_order(order_id: 'TXID-STALE')

    assert result.failure?
    assert_equal true, result.data[:not_found]
    assert_includes Array(result.data[:missing_ids]), 'TXID-STALE'
  end

  test 'get_orders lists IDs Kraken did not return under :missing instead of silently dropping them' do
    base = create(:asset, :bitcoin)
    quote = create(:asset, :usd)
    create(:ticker, exchange: @exchange, base_asset: base, quote_asset: quote, ticker: 'XBTUSD')

    @exchange.set_client
    # Kraken returns only TXID-A; TXID-STALE has aged out of QueryOrders retention.
    @exchange.send(:client).stubs(:query_orders_info).returns(
      Result::Success.new('TXID-A' => { raw: kraken_order_raw('XBTUSD', 'closed'), status: :closed })
    )
    # No fill in TradesHistory for TXID-STALE → it stays missing.
    @exchange.send(:client).stubs(:closed_orders_from_trades).returns(Result::Success.new({}))

    result = @exchange.get_orders(order_ids: %w[TXID-A TXID-STALE])

    assert result.success?
    assert_equal %w[TXID-A], result.data[:orders].keys
    assert_equal %w[TXID-STALE], result.data[:missing]
  end

  test 'authoritative_missing_orders? is true for Kraken (QueryOrders + TradesHistory)' do
    assert @exchange.authoritative_missing_orders?
  end

  test 'get_orders recovers a QueryOrders-missing order from TradesHistory as closed' do
    tx = waiting_kraken_tx('ODROP', base: 'BTC', quote: 'USD')
    @exchange.set_client
    client = @exchange.send(:client)
    client.stubs(:query_orders_info).returns(Result::Success.new({}))        # QueryOrders omits it
    client.expects(:closed_orders_from_trades)
          .with(order_ids: ['ODROP'], start: (tx.created_at - 1.hour).to_i)  # buffered bound
          .returns(Result::Success.new('ODROP' => {
                                         order_id: 'ODROP', status: :closed, side: :buy, order_type: :limit,
                                         price: BigDecimal('60000'), amount: nil, quote_amount: nil,
                                         amount_exec: BigDecimal('0.0005'), quote_amount_exec: BigDecimal('30'),
                                         fee: BigDecimal('0.05'), pair: 'XBTUSD', trade_count: 1,
                                         last_trade_at: 1.0, raw: { 'trades' => [] }
                                       }))

    result = @exchange.get_orders(order_ids: ['ODROP'])
    assert result.success?
    assert_empty result.data[:missing]
    recovered = result.data[:orders]['ODROP']
    assert_equal :closed, recovered[:status]
    assert_equal :limit_order, recovered[:order_type] # mapped from :limit
    assert_equal BigDecimal('30'), recovered[:quote_amount_exec]
  end

  test 'get_orders leaves an order missing when TradesHistory has no trades for it' do
    waiting_kraken_tx('OGONE')
    @exchange.set_client
    client = @exchange.send(:client)
    client.stubs(:query_orders_info).returns(Result::Success.new({}))
    client.stubs(:closed_orders_from_trades).returns(Result::Success.new({}))

    result = @exchange.get_orders(order_ids: ['OGONE'])
    assert_includes result.data[:missing], 'OGONE'
    assert_empty result.data[:orders]
  end

  test 'get_orders propagates a transient TradesHistory failure so the job can retry' do
    waiting_kraken_tx('OERR')
    @exchange.set_client
    client = @exchange.send(:client)
    client.stubs(:query_orders_info).returns(Result::Success.new({}))
    client.stubs(:closed_orders_from_trades).returns(Result::Failure.new('EService:Unavailable'))

    result = @exchange.get_orders(order_ids: ['OERR'])
    assert result.failure? # propagated, NOT swallowed → job funnels to typed retry
    assert_includes result.errors, 'EService:Unavailable'
  end

  test 'get_order recovers a QueryOrders-missing single order from TradesHistory' do
    tx = waiting_kraken_tx('OSOLO')
    @exchange.set_client
    client = @exchange.send(:client)
    client.stubs(:query_orders_info).returns(Result::Success.new({}))
    client.expects(:closed_orders_from_trades)
          .with(order_ids: ['OSOLO'], start: (tx.created_at - 1.hour).to_i)
          .returns(Result::Success.new('OSOLO' => {
                                         order_id: 'OSOLO', status: :closed, side: :buy, order_type: :limit,
                                         price: BigDecimal('60000'), amount: nil, quote_amount: nil,
                                         amount_exec: BigDecimal('0.0005'), quote_amount_exec: BigDecimal('30'),
                                         fee: BigDecimal('0.05'), pair: 'XBTUSD', trade_count: 1, last_trade_at: 1.0,
                                         raw: { 'trades' => [] }
                                       }))

    result = @exchange.get_order(order_id: 'OSOLO')
    assert result.success?
    assert_equal :closed, result.data[:status]
    assert_equal :limit_order, result.data[:order_type] # mapped, not :limit
  end

  test 'get_order propagates a transient TradesHistory failure' do
    waiting_kraken_tx('OSOLO2')
    @exchange.set_client
    client = @exchange.send(:client)
    client.stubs(:query_orders_info).returns(Result::Success.new({}))
    client.stubs(:closed_orders_from_trades).returns(Result::Failure.new('EService:Unavailable'))

    result = @exchange.get_order(order_id: 'OSOLO2')
    assert result.failure?
    assert_includes result.errors, 'EService:Unavailable'
  end

  # == transient_error? classification ==
  # Kraken returns some failures as HTTP 200 with an error array (e.g.
  # ["EGeneral:Internal error"], ["EAPI:Invalid nonce"]). These are transient and
  # should be retried, not failed loudly. transient_error? backs the conversion to
  # Client::TransientNetworkError in the fetch jobs, mirroring invalid_key_error?.

  test 'transient_error? is true for known transient Kraken codes' do
    assert @exchange.transient_error?(['EGeneral:Internal error'])
    assert @exchange.transient_error?(['EAPI:Invalid nonce'])
    assert @exchange.transient_error?(['EService:Unavailable'])
    assert @exchange.transient_error?(['EService:Busy'])
    assert @exchange.transient_error?(['EService:Deadline elapsed'])
  end

  test 'transient_error? matches a transient code embedded in a longer string' do
    assert @exchange.transient_error?(['Failed to fetch orders. Result: EGeneral:Internal error'])
  end

  test 'transient_error? is false for the temporary-lockout code (retrying worsens it)' do
    assert_not @exchange.transient_error?(['EGeneral:Temporary lockout'])
  end

  test 'transient_error? is false for non-transient codes' do
    assert_not @exchange.transient_error?(['EAPI:Invalid key'])
    assert_not @exchange.transient_error?(['EAPI:Insufficient funds'])
  end

  test 'transient_error? is false for the stale not_found sentence' do
    assert_not @exchange.transient_error?(['Kraken did not return data for order TXID-STALE'])
  end

  test 'transient_error? is false for empty and nil inputs' do
    assert_not @exchange.transient_error?([])
    assert_not @exchange.transient_error?(nil)
  end

  # Rate-limit codes (HTTP-200 "EAPI:Rate limit exceeded") are retryable, but on a
  # SEPARATE path from transient: they need a longer, escalating wait (retrying too
  # soon re-trips Kraken's decaying counter). throttled_error? backs the conversion to
  # Client::RateLimitedError in the fetch jobs, mirroring transient_error?.

  # Pin the throttle set EXACTLY: a broad matcher (e.g. 'Rate limit exceeded') or an
  # added EOrder:* code would silently widen auto-retry beyond the observed failure.
  test 'throttle known_errors is exactly the EAPI rate-limit code' do
    assert_equal ['EAPI:Rate limit exceeded'], @exchange.known_errors[:throttle]
  end

  test 'throttled_error? is true for the Kraken rate-limit code' do
    assert @exchange.throttled_error?(['EAPI:Rate limit exceeded'])
  end

  # EOrder:Rate limit exceeded is a trading-engine counter that never reaches the
  # query-order fetch jobs — it must NOT be classified as throttle here.
  test 'throttled_error? is false for the order-engine rate-limit code' do
    assert_not @exchange.throttled_error?(['EOrder:Rate limit exceeded'])
  end

  test 'throttled_error? matches a rate-limit code embedded in a longer string' do
    assert @exchange.throttled_error?(['Failed to fetch order 63. Result: EAPI:Rate limit exceeded'])
  end

  # The temporary-lockout code must stay out of BOTH retry paths — Kraken extends the
  # restriction if you keep calling while locked out (sequential invalid-key lockout).
  test 'temporary-lockout is neither throttled nor transient (must never auto-retry)' do
    assert_not @exchange.throttled_error?(['EGeneral:Temporary lockout'])
    assert_not @exchange.transient_error?(['EGeneral:Temporary lockout'])
  end

  test 'throttled_error? is false for transient and other non-throttle codes' do
    assert_not @exchange.throttled_error?(['EAPI:Invalid nonce'])
    assert_not @exchange.throttled_error?(['EAPI:Invalid key'])
    assert_not @exchange.throttled_error?(['exchange down'])
  end

  test 'throttled_error? is false for empty and nil inputs' do
    assert_not @exchange.throttled_error?([])
    assert_not @exchange.throttled_error?(nil)
  end

  private

  def kraken_order_raw(pair, status)
    {
      'descr' => { 'pair' => pair, 'ordertype' => 'market', 'type' => 'buy', 'price' => '0' },
      'price' => '50000',
      'cost' => '100',
      'vol' => '0.002',
      'vol_exec' => '0.002',
      'oflags' => '',
      'status' => status
    }
  end
end
