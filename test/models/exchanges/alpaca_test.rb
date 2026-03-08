require 'test_helper'

class Exchanges::AlpacaTest < ActiveSupport::TestCase
  setup do
    @exchange = create(:alpaca_exchange)
  end

  test 'coingecko_id returns nil' do
    assert_nil @exchange.coingecko_id
  end

  test 'known_errors includes insufficient_funds' do
    errors = @exchange.known_errors
    assert errors[:insufficient_funds].present?
    assert_kind_of Array, errors[:insufficient_funds]
  end

  test 'minimum_amount_logic returns quote' do
    assert_equal :quote, @exchange.minimum_amount_logic
  end

  test 'supports_withdrawal? returns false' do
    refute_predicate @exchange, :supports_withdrawal?
  end

  test 'requires_passphrase? returns true for mode selection' do
    assert_predicate @exchange, :requires_passphrase?
  end

  test 'set_client creates Clients::Alpaca instance' do
    api_key = stub(key: 'test_key', secret: 'test_secret', passphrase: nil)
    @exchange.set_client(api_key: api_key)
    assert_kind_of Clients::Alpaca, @exchange.instance_variable_get(:@client)
  end

  test 'set_client sets api_key reader' do
    api_key = stub(key: 'test_key', secret: 'test_secret', passphrase: nil)
    @exchange.set_client(api_key: api_key)
    assert_equal api_key, @exchange.api_key
  end

  test 'set_client handles nil api_key' do
    @exchange.set_client(api_key: nil)
    assert_nil @exchange.api_key
    assert_kind_of Clients::Alpaca, @exchange.instance_variable_get(:@client)
  end

  test 'market_open? returns true when clock says open' do
    Clients::Alpaca.any_instance.stubs(:get_clock).returns(Result::Success.new({ 'is_open' => true, 'next_open' => 1.hour.from_now.iso8601 }))
    assert_predicate @exchange, :market_open?
  end

  test 'market_open? returns false when clock says closed' do
    Clients::Alpaca.any_instance.stubs(:get_clock).returns(Result::Success.new({ 'is_open' => false, 'next_open' => 1.hour.from_now.iso8601 }))
    refute_predicate @exchange, :market_open?
  end

  test 'market_open? returns true when clock request fails' do
    Clients::Alpaca.any_instance.stubs(:get_clock).returns(Result::Failure.new('connection error'))
    assert_predicate @exchange, :market_open?
  end

  test 'set_client defaults to paper mode when passphrase is nil' do
    api_key = stub(key: 'test_key', secret: 'test_secret', passphrase: nil)
    @exchange.set_client(api_key: api_key)
    client = @exchange.instance_variable_get(:@client)
    assert client.instance_variable_get(:@paper)
  end

  test 'set_client uses live mode when passphrase is live' do
    api_key = stub(key: 'test_key', secret: 'test_secret', passphrase: 'live')
    @exchange.set_client(api_key: api_key)
    client = @exchange.instance_variable_get(:@client)
    refute client.instance_variable_get(:@paper)
  end

  test 'fetch_withdrawal_fees! returns empty success' do
    result = @exchange.fetch_withdrawal_fees!
    assert_predicate result, :success?
    assert_equal({}, result.data)
  end
end
