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

  test 'minimum_amount_logic returns base_or_quote' do
    assert_equal :base_or_quote, @exchange.minimum_amount_logic
  end

  test 'set_client creates a Clients::Bitvavo instance' do
    @exchange.set_client
    assert_kind_of Clients::Bitvavo, @exchange.send(:client)
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

    Clients::Bitvavo.any_instance.stubs(:cancel_order).returns(
      Result::Failure.new('Order not found')
    )
    Clients::Bitvavo.any_instance.expects(:balance).never

    result = @exchange.get_api_key_validity(api_key: api_key)
    assert result.success?
    assert_equal true, result.data
  end

  test 'get_api_key_validity uses balance for withdrawal keys' do
    Rails.configuration.stubs(:dry_run).returns(false)
    api_key = create(:api_key, exchange: @exchange, key_type: :withdrawal, key: 'test_key', secret: 'test_secret')

    Clients::Bitvavo.any_instance.stubs(:balance).returns(
      Result::Success.new([{ 'symbol' => 'BTC', 'available' => '0.5' }])
    )
    Clients::Bitvavo.any_instance.expects(:cancel_order).never

    result = @exchange.get_api_key_validity(api_key: api_key)
    assert result.success?
    assert_equal true, result.data
  end

  test 'get_api_key_validity returns false for invalid trading key' do
    Rails.configuration.stubs(:dry_run).returns(false)
    api_key = create(:api_key, exchange: @exchange, key_type: :trading, key: 'bad_key', secret: 'bad_secret')

    Clients::Bitvavo.any_instance.stubs(:cancel_order).returns(
      Result::Failure.new('Invalid API key.', data: { status: 401 })
    )

    result = @exchange.get_api_key_validity(api_key: api_key)
    assert result.success?
    assert_equal false, result.data
  end
end
