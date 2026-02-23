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
end
