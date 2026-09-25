require 'test_helper'

class Exchanges::CoinbaseTest < ActiveSupport::TestCase
  setup do
    @exchange = create(:coinbase_exchange)
    Rails.configuration.stubs(:dry_run).returns(false)
  end

  test 'get_api_key_validity validates trading key permissions' do
    api_key = create(:api_key, exchange: @exchange, key_type: :trading, key: 'test_key', secret: 'test_secret')

    Honeymaker::Clients::Coinbase.any_instance.stubs(:get_api_key_permissions).returns(
      Result::Success.new({ 'can_view' => true, 'can_trade' => true, 'can_transfer' => false })
    )

    result = @exchange.get_api_key_validity(api_key: api_key)
    assert result.success?
    assert_equal true, result.data
  end

  test 'get_api_key_validity rejects trading key with transfer permissions' do
    api_key = create(:api_key, exchange: @exchange, key_type: :trading, key: 'test_key', secret: 'test_secret')

    Honeymaker::Clients::Coinbase.any_instance.stubs(:get_api_key_permissions).returns(
      Result::Success.new({ 'can_view' => true, 'can_trade' => false, 'can_transfer' => true })
    )

    result = @exchange.get_api_key_validity(api_key: api_key)
    assert result.success?
    assert_equal false, result.data
  end

  test 'get_api_key_validity validates withdrawal key permissions' do
    api_key = create(:api_key, exchange: @exchange, key_type: :withdrawal, key: 'test_key', secret: 'test_secret')

    Honeymaker::Clients::Coinbase.any_instance.stubs(:get_api_key_permissions).returns(
      Result::Success.new({ 'can_view' => true, 'can_trade' => false, 'can_transfer' => true })
    )

    result = @exchange.get_api_key_validity(api_key: api_key)
    assert result.success?
    assert_equal true, result.data
  end

  test 'get_api_key_validity rejects withdrawal key with trade permissions' do
    api_key = create(:api_key, exchange: @exchange, key_type: :withdrawal, key: 'test_key', secret: 'test_secret')

    Honeymaker::Clients::Coinbase.any_instance.stubs(:get_api_key_permissions).returns(
      Result::Success.new({ 'can_view' => true, 'can_trade' => true, 'can_transfer' => false })
    )

    result = @exchange.get_api_key_validity(api_key: api_key)
    assert result.success?
    assert_equal false, result.data
  end

  test 'get_api_key_validity returns false for invalid key' do
    api_key = create(:api_key, exchange: @exchange, key_type: :withdrawal, key: 'bad_key', secret: 'bad_secret')

    Honeymaker::Clients::Coinbase.any_instance.stubs(:get_api_key_permissions).returns(
      Result::Failure.new('Unauthorized', data: { status: 401 })
    )

    result = @exchange.get_api_key_validity(api_key: api_key)
    assert result.success?
    assert_equal false, result.data
  end

  test 'get_api_key_validity handles non-HTTP errors without raising' do
    api_key = create(:api_key, exchange: @exchange, key_type: :trading, key: 'test_key', secret: 'test_secret')

    Honeymaker::Clients::Coinbase.any_instance.stubs(:get_api_key_permissions).returns(
      Result::Failure.new('Connection reset by peer')
    )

    result = @exchange.get_api_key_validity(api_key: api_key)
    assert result.failure?
  end

  # key_permissions sometimes answers 500 for a good key. It is asked again; what it never does is
  # place an order to find out — the old fallback bought $1 of BTC-USD, and filled whenever the account
  # held a dollar.
  def permissions_answers(*answers)
    Honeymaker::Clients::Coinbase.any_instance.stubs(:get_api_key_permissions).returns(*answers)
    Honeymaker::Clients::Coinbase.any_instance.expects(:create_order).never
    @exchange.stubs(:sleep)
  end

  def server_error = Result::Failure.new('Internal Server Error', data: { status: 500 })

  test 'a 500 is asked again, and the next answer decides' do
    trading = create(:api_key, exchange: @exchange, key_type: :trading)
    permissions_answers(server_error, Result::Success.new({ 'can_view' => true, 'can_trade' => true, 'can_transfer' => false }))
    assert_equal true, @exchange.get_api_key_validity(api_key: trading).data

    withdrawal = create(:api_key, exchange: @exchange, key_type: :withdrawal)
    permissions_answers(server_error, Result::Success.new({ 'can_view' => true, 'can_trade' => true, 'can_transfer' => true }))
    assert_equal false, @exchange.get_api_key_validity(api_key: withdrawal).data
  end

  test 'a 500 followed by a 401 is a rejected key' do
    permissions_answers(server_error, Result::Failure.new('Unauthorized', data: { status: 401 }))

    assert_equal false, @exchange.get_api_key_validity(api_key: create(:api_key, exchange: @exchange)).data
  end

  test 'a 500 followed by another failure returns that failure' do
    permissions_answers(server_error, Result::Failure.new('Connection reset by peer'))

    result = @exchange.get_api_key_validity(api_key: create(:api_key, exchange: @exchange))
    assert result.failure?
    assert_equal ['Connection reset by peer'], result.errors
  end

  test 'a persistent 500 is inconclusive after three asks, and nothing is inferred' do
    Honeymaker::Clients::Coinbase.any_instance.expects(:get_api_key_permissions).times(3).returns(server_error)
    Honeymaker::Clients::Coinbase.any_instance.expects(:create_order).never
    Honeymaker::Clients::Coinbase.any_instance.expects(:list_accounts).never
    @exchange.stubs(:sleep)

    assert_predicate @exchange.get_api_key_validity(api_key: create(:api_key, exchange: @exchange)), :failure?
  end

  # The form path: an inconclusive answer is a "could not verify", and the stored key stays as it was.
  test 'a replacement Coinbase cannot verify leaves the stored key as it was' do
    api_key = create(:api_key, exchange: @exchange, key_type: :trading, status: :correct)
    stored = api_key.key
    Honeymaker::Clients::Coinbase.any_instance.stubs(:get_api_key_permissions).returns(server_error)
    @exchange.stubs(:sleep)
    api_key.stubs(:exchange).returns(@exchange)

    api_key.validate_credentials!(key: 'new-key', secret: 'new-secret')

    assert_predicate api_key, :pending_validation?
    assert_equal stored, api_key.reload.key
    assert_predicate api_key, :correct?
  end

  # == get_orders shape contract ==
  # Bulk-list pattern: Coinbase fetches by IDs in batches. The contract is
  # { orders:, missing: } so callers can react to dropped IDs uniformly.

  test 'get_orders returns { orders:, missing: [] } shape when every requested ID is returned' do
    # Stub parse_order_data so this test focuses on the shape contract,
    # not the Coinbase-specific payload parsing.
    @exchange.stubs(:parse_order_data).returns(stub_parsed_order)

    Honeymaker::Clients::Coinbase.any_instance.stubs(:list_orders).returns(
      Result::Success.new('orders' => [{ 'order_id' => 'order-1' }, { 'order_id' => 'order-2' }])
    )

    result = @exchange.get_orders(order_ids: %w[order-1 order-2])

    assert result.success?
    assert_equal %i[orders missing].sort, result.data.keys.sort
    assert_equal %w[order-1 order-2].sort, result.data[:orders].keys.sort
    assert_equal [], result.data[:missing]
  end

  test 'get_orders reports requested IDs absent from the Coinbase response under :missing' do
    @exchange.stubs(:parse_order_data).returns(stub_parsed_order)

    # Coinbase's list_orders is a bulk endpoint — if Coinbase drops/omits an ID
    # from the response, the contract requires it to surface under :missing
    # instead of being silently lost.
    Honeymaker::Clients::Coinbase.any_instance.stubs(:list_orders).returns(
      Result::Success.new('orders' => [{ 'order_id' => 'order-1' }])
    )

    result = @exchange.get_orders(order_ids: %w[order-1 order-stale])

    assert result.success?
    assert_equal %w[order-1], result.data[:orders].keys
    assert_equal %w[order-stale], result.data[:missing]
  end

  private

  def stub_parsed_order
    { status: :closed, amount: 0.002, quote_amount: 100, side: :buy, order_type: :market_order }
  end
end
