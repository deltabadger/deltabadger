require 'test_helper'

class GetExchangeBalancesToolTest < ActiveSupport::TestCase
  setup do
    @user = create(:user, admin: true)
    @exchange = create(:binance_exchange)
    @api_key = create(:api_key, user: @user, exchange: @exchange, status: :correct)
    ActionMCP::Current.stubs(:user).returns(@user)
  end

  test 'fetches balances from exchange' do
    btc = create(:asset, :bitcoin)
    usd = create(:asset, :usd)
    balances = {
      btc.id => { free: 0.5, locked: 0.1 },
      usd.id => { free: 1000.0, locked: 0.0 }
    }
    Exchanges::Binance.any_instance.stubs(:set_client)
    Exchanges::Binance.any_instance.stubs(:get_balances).returns(Result::Success.new(balances))

    response = GetExchangeBalancesTool.call('exchange_name' => 'Binance')
    text = response.contents.first.text

    assert_match(/Binance Balances/, text)
    assert_match(/BTC: 0.5/, text)
    assert_match(/0.1 locked/, text)
    assert_match(/USD: 1000.0/, text)
  end

  test 'handles missing exchange' do
    response = GetExchangeBalancesTool.call('exchange_name' => 'NonExistent')
    text = response.contents.first.text

    assert_match(/not found/, text)
  end

  test 'handles missing API key' do
    @api_key.destroy
    response = GetExchangeBalancesTool.call('exchange_name' => 'Binance')
    text = response.contents.first.text

    assert_match(/No valid API key/, text)
  end

  test 'handles exchange API failure' do
    Exchanges::Binance.any_instance.stubs(:set_client)
    Exchanges::Binance.any_instance.stubs(:get_balances).returns(Result::Failure.new('Connection timeout'))

    response = GetExchangeBalancesTool.call('exchange_name' => 'Binance')
    text = response.contents.first.text

    assert_match(/Failed to fetch balances/, text)
    assert_match(/Connection timeout/, text)
  end

  test 'reports all zero balances' do
    btc = create(:asset, :bitcoin)
    balances = {
      btc.id => { free: 0.0, locked: 0.0 }
    }
    Exchanges::Binance.any_instance.stubs(:set_client)
    Exchanges::Binance.any_instance.stubs(:get_balances).returns(Result::Success.new(balances))

    response = GetExchangeBalancesTool.call('exchange_name' => 'Binance')
    text = response.contents.first.text

    assert_match(/All balances on Binance are zero/, text)
  end

  test 'is case-insensitive for exchange name' do
    btc = create(:asset, :bitcoin)
    balances = { btc.id => { free: 1.0, locked: 0.0 } }
    Exchanges::Binance.any_instance.stubs(:set_client)
    Exchanges::Binance.any_instance.stubs(:get_balances).returns(Result::Success.new(balances))

    response = GetExchangeBalancesTool.call('exchange_name' => 'binance')
    text = response.contents.first.text

    assert_match(/Binance Balances/, text)
  end

  test 'skips incorrect API keys' do
    @api_key.update!(status: :incorrect)

    response = GetExchangeBalancesTool.call('exchange_name' => 'Binance')
    text = response.contents.first.text

    assert_match(/No valid API key/, text)
  end
end
