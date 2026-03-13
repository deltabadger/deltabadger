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

  # list_withdrawal_addresses includes key field

  test 'list_withdrawal_addresses includes key field from API response' do
    asset = create(:asset, :bitcoin)
    create(:ticker, exchange: @exchange, base_asset: asset, quote_asset: create(:asset, :usd))

    Clients::Kraken.any_instance.stubs(:get_withdraw_addresses).returns(
      Result::Success.new(
        'error' => [],
        'result' => [
          { 'address' => 'bc1q...abc', 'key' => 'My BTC Wallet', 'method' => 'Bitcoin', 'verified' => true },
          { 'address' => 'bc1q...def', 'key' => 'Cold Storage', 'method' => 'Bitcoin', 'verified' => true },
          { 'address' => 'bc1q...unverified', 'key' => 'Unverified', 'method' => 'Bitcoin', 'verified' => false }
        ]
      )
    )

    addresses = @exchange.list_withdrawal_addresses(asset: asset)

    assert_equal 2, addresses.size
    assert_equal 'bc1q...abc', addresses[0][:name]
    assert_equal 'My BTC Wallet', addresses[0][:key]
    assert_equal 'bc1q...abc - My BTC Wallet - Bitcoin', addresses[0][:label]
    assert_equal 'bc1q...def', addresses[1][:name]
    assert_equal 'Cold Storage', addresses[1][:key]
  end

  # withdraw looks up key name

  test 'withdraw passes key name instead of raw address to API' do
    asset = create(:asset, :bitcoin)
    create(:ticker, exchange: @exchange, base_asset: asset, quote_asset: create(:asset, :usd))

    api_key = create(:api_key, exchange: @exchange, key_type: :withdrawal, key: 'test_key', secret: 'dGVzdF9zZWNyZXQ=')
    @exchange.set_client(api_key: api_key)

    Clients::Kraken.any_instance.stubs(:get_withdraw_addresses).returns(
      Result::Success.new(
        'error' => [],
        'result' => [
          { 'address' => 'bc1q...abc', 'key' => 'My BTC Wallet', 'method' => 'Bitcoin', 'verified' => true }
        ]
      )
    )

    Clients::Kraken.any_instance.expects(:withdraw).with(
      asset: 'BTC',
      key: 'My BTC Wallet',
      amount: '0.5',
      address: 'bc1q...abc'
    ).returns(Result::Success.new({ 'error' => [], 'result' => { 'refid' => 'ATEST' } }))

    result = @exchange.withdraw(asset: asset, amount: BigDecimal('0.5'), address: 'bc1q...abc')

    assert result.success?
    assert_equal 'ATEST', result.data[:withdrawal_id]
  end

  test 'withdraw falls back to address when key name not found' do
    asset = create(:asset, :bitcoin)
    create(:ticker, exchange: @exchange, base_asset: asset, quote_asset: create(:asset, :usd))

    api_key = create(:api_key, exchange: @exchange, key_type: :withdrawal, key: 'test_key', secret: 'dGVzdF9zZWNyZXQ=')
    @exchange.set_client(api_key: api_key)

    Clients::Kraken.any_instance.stubs(:get_withdraw_addresses).returns(
      Result::Success.new('error' => [], 'result' => [])
    )

    Clients::Kraken.any_instance.expects(:withdraw).with(
      asset: 'BTC',
      key: 'bc1q...unknown',
      amount: '0.5',
      address: 'bc1q...unknown'
    ).returns(Result::Success.new({ 'error' => [], 'result' => { 'refid' => 'BTEST' } }))

    result = @exchange.withdraw(asset: asset, amount: BigDecimal('0.5'), address: 'bc1q...unknown')

    assert result.success?
  end
end
