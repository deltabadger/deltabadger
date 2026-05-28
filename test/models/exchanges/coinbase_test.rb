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

  test 'get_api_key_validity falls back to probing when key_permissions returns 500' do
    api_key = create(:api_key, exchange: @exchange, key_type: :trading, key: 'test_key', secret: 'test_secret')

    Honeymaker::Clients::Coinbase.any_instance.stubs(:get_api_key_permissions).returns(
      Result::Failure.new('Internal Server Error', data: { status: 500 })
    )
    Honeymaker::Clients::Coinbase.any_instance.stubs(:list_accounts).returns(
      Result::Success.new({ 'accounts' => [] })
    )
    # Order rejected for insufficient funds (HTTP 200, success=false) proves trade permission
    Honeymaker::Clients::Coinbase.any_instance.stubs(:create_order).returns(
      Result::Success.new({ 'success' => false, 'error_response' => { 'error' => 'INSUFFICIENT_FUND' } })
    )

    result = @exchange.get_api_key_validity(api_key: api_key)
    assert result.success?
    assert_equal true, result.data
  end

  test 'get_api_key_validity returns false when key_permissions returns 500 and list_accounts fails' do
    api_key = create(:api_key, exchange: @exchange, key_type: :trading, key: 'test_key', secret: 'test_secret')

    Honeymaker::Clients::Coinbase.any_instance.stubs(:get_api_key_permissions).returns(
      Result::Failure.new('Internal Server Error', data: { status: 500 })
    )
    Honeymaker::Clients::Coinbase.any_instance.stubs(:list_accounts).returns(
      Result::Failure.new('Unauthorized', data: { status: 401 })
    )

    result = @exchange.get_api_key_validity(api_key: api_key)
    assert result.success?
    assert_equal false, result.data
  end

  test 'get_api_key_validity returns false when key_permissions returns 500 and order returns 403' do
    api_key = create(:api_key, exchange: @exchange, key_type: :trading, key: 'test_key', secret: 'test_secret')

    Honeymaker::Clients::Coinbase.any_instance.stubs(:get_api_key_permissions).returns(
      Result::Failure.new('Internal Server Error', data: { status: 500 })
    )
    Honeymaker::Clients::Coinbase.any_instance.stubs(:list_accounts).returns(
      Result::Success.new({ 'accounts' => [] })
    )
    # HTTP 403 means no trade permission
    Honeymaker::Clients::Coinbase.any_instance.stubs(:create_order).returns(
      Result::Failure.new('Forbidden', data: { status: 403 })
    )

    result = @exchange.get_api_key_validity(api_key: api_key)
    assert result.success?
    assert_equal false, result.data
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
