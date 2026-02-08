require 'test_helper'

class Exchanges::BybitTest < ActiveSupport::TestCase
  setup do
    @exchange = create(:bybit_exchange)
  end

  test 'coingecko_id returns bybit_spot' do
    assert_equal 'bybit_spot', @exchange.coingecko_id
  end

  test 'known_errors returns expected error messages' do
    errors = @exchange.known_errors
    assert errors[:insufficient_funds].present?
    assert errors[:invalid_key].present?
    assert_includes errors[:insufficient_funds], '170131'
    assert_includes errors[:invalid_key], '10003'
  end

  test 'minimum_amount_logic returns base_or_quote' do
    assert_equal :base_or_quote, @exchange.minimum_amount_logic
  end

  test 'set_client creates a Clients::Bybit instance' do
    @exchange.set_client
    assert_kind_of Clients::Bybit, @exchange.send(:client)
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
