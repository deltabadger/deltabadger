require 'test_helper'

class Exchanges::BitrueTest < ActiveSupport::TestCase
  setup do
    @exchange = create(:bitrue_exchange)
  end

  test 'coingecko_id returns bitrue' do
    assert_equal 'bitrue', @exchange.coingecko_id
  end

  test 'known_errors returns expected error messages' do
    errors = @exchange.known_errors
    assert errors[:insufficient_funds].present?
    assert errors[:invalid_key].present?
    assert_includes errors[:insufficient_funds], 'Insufficient balance.'
    assert_includes errors[:invalid_key], 'Invalid Api-Key ID.'
  end

  test 'minimum_amount_logic returns base_and_quote for market orders' do
    assert_equal :base_and_quote, @exchange.minimum_amount_logic(order_type: :market_order)
  end

  test 'minimum_amount_logic returns base_and_quote_in_base for limit orders' do
    assert_equal :base_and_quote_in_base, @exchange.minimum_amount_logic(order_type: :limit_order)
  end

  test 'set_client creates a Clients::Bitrue instance' do
    @exchange.set_client
    assert_kind_of Clients::Bitrue, @exchange.send(:client)
  end

  test 'set_client with api_key stores the api_key' do
    api_key = create(:api_key, exchange: @exchange)
    @exchange.set_client(api_key: api_key)
    assert_equal api_key, @exchange.api_key
  end

  test 'requires_passphrase? returns false' do
    assert_equal false, @exchange.requires_passphrase?
  end
end
