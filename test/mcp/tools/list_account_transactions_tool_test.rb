require 'test_helper'

class ListAccountTransactionsToolTest < ActiveSupport::TestCase
  setup do
    @user = create(:user, admin: true)
    ActionMCP::Current.stubs(:user).returns(@user)
    @exchange = create(:binance_exchange)
    @api_key = create(:api_key, user: @user, exchange: @exchange, status: :correct)
  end

  test 'lists account transactions' do
    create(:account_transaction, user: @user, api_key: @api_key, exchange: @exchange)

    response = ListAccountTransactionsTool.call
    text = response.contents.first.text

    assert_match(/Account transactions \(1\)/, text)
    assert_match(/BUY/, text)
    assert_match(/BTC/, text)
  end

  test 'returns empty message when no transactions' do
    response = ListAccountTransactionsTool.call
    text = response.contents.first.text

    assert_equal 'No account transactions found.', text
  end

  test 'respects limit parameter' do
    3.times { create(:account_transaction, user: @user, api_key: @api_key, exchange: @exchange) }

    response = ListAccountTransactionsTool.call('limit' => 2)
    text = response.contents.first.text

    assert_match(/Account transactions \(2\)/, text)
  end

  test 'filters by entry_type' do
    create(:account_transaction, user: @user, api_key: @api_key, exchange: @exchange, entry_type: :buy)
    create(:account_transaction, :sell, user: @user, api_key: @api_key, exchange: @exchange)

    response = ListAccountTransactionsTool.call('entry_type' => 'sell')
    text = response.contents.first.text

    assert_match(/Account transactions \(1\)/, text)
    assert_match(/SELL/, text)
  end

  test 'filters by exchange_id' do
    exchange2 = create(:kraken_exchange)
    api_key2 = create(:api_key, user: @user, exchange: exchange2, status: :correct)
    create(:account_transaction, user: @user, api_key: @api_key, exchange: @exchange, base_currency: 'BTC')
    create(:account_transaction, user: @user, api_key: api_key2, exchange: exchange2, base_currency: 'ETH')

    response = ListAccountTransactionsTool.call('exchange_id' => @exchange.id)
    text = response.contents.first.text

    assert_match(/Account transactions \(1\)/, text)
    assert_match(/BTC/, text)
  end

  test 'returns error for invalid exchange' do
    response = ListAccountTransactionsTool.call('exchange_id' => 99_999)
    text = response.contents.first.text

    assert_match(/Exchange not found/, text)
  end
end
