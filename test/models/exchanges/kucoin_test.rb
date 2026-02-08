require 'test_helper'

class Exchanges::KucoinTest < ActiveSupport::TestCase
  setup do
    @exchange = create(:kucoin_exchange)
  end

  test 'coingecko_id returns kucoin' do
    assert_equal 'kucoin', @exchange.coingecko_id
  end

  test 'known_errors includes insufficient_funds and invalid_key' do
    errors = @exchange.known_errors
    assert errors[:insufficient_funds].present?
    assert errors[:invalid_key].present?
    assert_kind_of Array, errors[:insufficient_funds]
    assert_kind_of Array, errors[:invalid_key]
  end

  test 'minimum_amount_logic returns base_or_quote' do
    assert_equal :base_or_quote, @exchange.minimum_amount_logic
  end

  test 'requires_passphrase? returns true' do
    assert_predicate @exchange, :requires_passphrase?
  end

  test 'set_client creates Clients::Kucoin instance' do
    api_key = stub(key: 'test_key', secret: 'test_secret', passphrase: 'test_pass')
    @exchange.set_client(api_key: api_key)
    assert_kind_of Clients::Kucoin, @exchange.instance_variable_get(:@client)
  end

  test 'set_client sets api_key reader' do
    api_key = stub(key: 'test_key', secret: 'test_secret', passphrase: 'test_pass')
    @exchange.set_client(api_key: api_key)
    assert_equal api_key, @exchange.api_key
  end

  test 'set_client handles nil api_key' do
    @exchange.set_client(api_key: nil)
    assert_nil @exchange.api_key
    assert_kind_of Clients::Kucoin, @exchange.instance_variable_get(:@client)
  end
end
