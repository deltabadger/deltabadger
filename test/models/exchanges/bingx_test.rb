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

  test 'minimum_amount_logic returns base_and_quote' do
    assert_equal :base_and_quote, @exchange.minimum_amount_logic
  end

  test 'set_client creates a Clients::Bingx instance' do
    @exchange.set_client
    assert_kind_of Clients::Bingx, @exchange.send(:client)
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

    Clients::Bingx.any_instance.stubs(:cancel_order).returns(
      Result::Success.new({ 'code' => 100_400, 'msg' => 'Order does not exist' })
    )
    Clients::Bingx.any_instance.expects(:get_balances).never

    result = @exchange.get_api_key_validity(api_key: api_key)
    assert result.success?
    assert_equal true, result.data
  end

  test 'get_api_key_validity uses get_balances for withdrawal keys' do
    Rails.configuration.stubs(:dry_run).returns(false)
    api_key = create(:api_key, exchange: @exchange, key_type: :withdrawal, key: 'test_key', secret: 'test_secret')

    Clients::Bingx.any_instance.stubs(:get_balances).returns(
      Result::Success.new({ 'code' => 0, 'data' => [] })
    )
    Clients::Bingx.any_instance.expects(:cancel_order).never

    result = @exchange.get_api_key_validity(api_key: api_key)
    assert result.success?
    assert_equal true, result.data
  end

  test 'get_api_key_validity returns false for invalid trading key' do
    Rails.configuration.stubs(:dry_run).returns(false)
    api_key = create(:api_key, exchange: @exchange, key_type: :trading, key: 'bad_key', secret: 'bad_secret')

    Clients::Bingx.any_instance.stubs(:cancel_order).returns(
      Result::Success.new({ 'code' => 100_001, 'msg' => 'Invalid Api-Key ID' })
    )

    result = @exchange.get_api_key_validity(api_key: api_key)
    assert result.success?
    assert_equal false, result.data
  end
end
