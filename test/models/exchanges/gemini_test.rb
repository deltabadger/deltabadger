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
end
