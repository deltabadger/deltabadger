require 'test_helper'

class Exchanges::BitmartTest < ActiveSupport::TestCase
  setup do
    @exchange = create(:bitmart_exchange)
  end

  test 'coingecko_id returns bitmart' do
    assert_equal 'bitmart', @exchange.coingecko_id
  end

  test 'known_errors returns expected error messages' do
    errors = @exchange.known_errors
    assert errors[:insufficient_funds].present?
    assert errors[:invalid_key].present?
    assert_includes errors[:insufficient_funds], 'Balance not enough'
    assert_includes errors[:invalid_key], 'Invalid ACCESS_KEY'
  end

  test 'minimum_amount_logic returns base_and_quote' do
    assert_equal :base_and_quote, @exchange.minimum_amount_logic
  end

  test 'set_client creates a Honeymaker BitMart client' do
    @exchange.set_client
    assert_kind_of Honeymaker::Clients::BitMart, @exchange.send(:client)
  end

  test 'set_client with api_key stores the api_key' do
    api_key = create(:api_key, exchange: @exchange, raw_passphrase: 'test_memo')
    @exchange.set_client(api_key: api_key)
    assert_equal api_key, @exchange.api_key
  end

  test 'requires_passphrase? returns true' do
    assert_equal true, @exchange.requires_passphrase?
  end

  test 'get_api_key_validity uses cancel_order for trading keys' do
    Rails.configuration.stubs(:dry_run).returns(false)
    api_key = create(:api_key, exchange: @exchange, key_type: :trading, key: 'test_key', secret: 'test_secret',
                               raw_passphrase: 'test_memo')

    Honeymaker::Clients::BitMart.any_instance.stubs(:cancel_order).returns(
      Result::Success.new({ 'code' => 50_030, 'message' => 'Order not found' })
    )
    Honeymaker::Clients::BitMart.any_instance.expects(:get_wallet).never

    result = @exchange.get_api_key_validity(api_key: api_key)
    assert result.success?
    assert_equal true, result.data
  end

  test 'get_api_key_validity uses get_wallet for withdrawal keys' do
    Rails.configuration.stubs(:dry_run).returns(false)
    api_key = create(:api_key, exchange: @exchange, key_type: :withdrawal, key: 'test_key', secret: 'test_secret',
                               raw_passphrase: 'test_memo')

    Honeymaker::Clients::BitMart.any_instance.stubs(:get_wallet).returns(
      Result::Success.new({ 'code' => 1000, 'data' => { 'wallet' => [] } })
    )
    Honeymaker::Clients::BitMart.any_instance.expects(:cancel_order).never

    result = @exchange.get_api_key_validity(api_key: api_key)
    assert result.success?
    assert_equal true, result.data
  end

  test 'get_api_key_validity returns false for invalid trading key' do
    Rails.configuration.stubs(:dry_run).returns(false)
    api_key = create(:api_key, exchange: @exchange, key_type: :trading, key: 'bad_key', secret: 'bad_secret',
                               raw_passphrase: 'test_memo')

    Honeymaker::Clients::BitMart.any_instance.stubs(:cancel_order).returns(
      Result::Success.new({ 'code' => 30_006, 'message' => 'Invalid ACCESS_KEY' })
    )

    result = @exchange.get_api_key_validity(api_key: api_key)
    assert result.success?
    assert_equal false, result.data
  end
end
