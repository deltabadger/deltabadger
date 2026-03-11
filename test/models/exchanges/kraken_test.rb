require 'test_helper'

class Exchanges::KrakenTest < ActiveSupport::TestCase
  setup do
    @exchange = create(:kraken_exchange)
    Rails.configuration.stubs(:dry_run).returns(false)
  end

  test 'get_api_key_validity uses add_order for trading keys' do
    api_key = create(:api_key, exchange: @exchange, key_type: :trading, key: 'test_key', secret: 'dGVzdF9zZWNyZXQ=')

    Clients::Kraken.any_instance.stubs(:add_order).returns(
      Result::Success.new({ 'error' => [] })
    )
    Clients::Kraken.any_instance.expects(:get_extended_balance).never

    result = @exchange.get_api_key_validity(api_key: api_key)
    assert result.success?
    assert_equal true, result.data
  end

  test 'get_api_key_validity uses get_extended_balance for withdrawal keys' do
    api_key = create(:api_key, exchange: @exchange, key_type: :withdrawal, key: 'test_key', secret: 'dGVzdF9zZWNyZXQ=')

    Clients::Kraken.any_instance.stubs(:get_extended_balance).returns(
      Result::Success.new({ 'error' => [], 'result' => {} })
    )
    Clients::Kraken.any_instance.expects(:add_order).never

    result = @exchange.get_api_key_validity(api_key: api_key)
    assert result.success?
    assert_equal true, result.data
  end

  test 'get_api_key_validity returns incorrect for invalid withdrawal key' do
    api_key = create(:api_key, exchange: @exchange, key_type: :withdrawal, key: 'bad_key', secret: 'dGVzdF9zZWNyZXQ=')

    Clients::Kraken.any_instance.stubs(:get_extended_balance).returns(
      Result::Success.new({ 'error' => ['EAPI:Invalid key'] })
    )

    result = @exchange.get_api_key_validity(api_key: api_key)
    assert result.success?
    assert_equal false, result.data
  end

  test 'get_api_key_validity returns incorrect for permission denied on withdrawal key' do
    api_key = create(:api_key, exchange: @exchange, key_type: :withdrawal, key: 'bad_key', secret: 'dGVzdF9zZWNyZXQ=')

    Clients::Kraken.any_instance.stubs(:get_extended_balance).returns(
      Result::Success.new({ 'error' => ['EGeneral:Permission denied'] })
    )

    result = @exchange.get_api_key_validity(api_key: api_key)
    assert result.success?
    assert_equal false, result.data
  end
end
