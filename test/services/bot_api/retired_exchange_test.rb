require 'test_helper'

# The REST/MCP surfaces resolve an exchange by name or id with no scope at all, so a retired venue
# stays reachable there long after it has disappeared from every picker in the UI.
class BotApiRetiredExchangeTest < ActiveSupport::TestCase
  setup do
    @user = create(:user)
    @retired = Exchanges::Bitmart.create!(name: 'Bitmart', available: false)
  end

  test 'Lookup.find_exchange does not resolve a retired exchange' do
    assert_nil BotApi::Orders::Lookup.find_exchange('bitmart')
  end

  test 'Lookup.find_exchange still resolves a live exchange' do
    live = create(:binance_exchange)

    assert_equal live, BotApi::Orders::Lookup.find_exchange(live.name)
  end

  test 'balances refuse a retired exchange by name' do
    result = BotApi::Exchanges::Balances.call(user: @user, exchange_name: 'Bitmart')

    assert_equal :not_found, result.status
    assert_equal 'exchange_not_found', result.error_code
  end

  test 'balances refuse a retired exchange by id' do
    result = BotApi::Exchanges::Balances.call(user: @user, exchange_id: @retired.id)

    assert_equal :not_found, result.status
    assert_equal 'exchange_not_found', result.error_code
  end

  test 'bot creation refuses a retired exchange' do
    result = BotApi::Bots::Create.call(user: @user, exchange_name: 'Bitmart', base_asset: 'BTC',
                                       quote_asset: 'USD', quote_amount: 100, interval: 'day')

    assert_equal :not_found, result.status
    assert_equal 'exchange_not_found', result.error_code
  end
end
