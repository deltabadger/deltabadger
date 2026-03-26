require 'test_helper'

class ExportTransactionsCsvToolTest < ActiveSupport::TestCase
  setup do
    @user = create(:user, admin: true)
    ActionMCP::Current.stubs(:user).returns(@user)
  end

  test 'exports transactions as CSV' do
    exchange = create(:binance_exchange)
    api_key = create(:api_key, user: @user, exchange: exchange, status: :correct)
    create(:account_transaction, user: @user, api_key: api_key, exchange: exchange)

    response = ExportTransactionsCsvTool.call
    text = response.contents.first.text

    assert_match(/date/, text)
    assert_match(/BTC/, text)
  end

  test 'returns empty message when no transactions' do
    response = ExportTransactionsCsvTool.call
    text = response.contents.first.text

    assert_equal 'No account transactions found matching the filters.', text
  end

  test 'filters by exchange_id' do
    exchange1 = create(:binance_exchange)
    exchange2 = create(:kraken_exchange)
    api_key1 = create(:api_key, user: @user, exchange: exchange1, status: :correct)
    api_key2 = create(:api_key, user: @user, exchange: exchange2, status: :correct)
    create(:account_transaction, user: @user, api_key: api_key1, exchange: exchange1, base_currency: 'BTC')
    create(:account_transaction, user: @user, api_key: api_key2, exchange: exchange2, base_currency: 'ETH')

    response = ExportTransactionsCsvTool.call('exchange_id' => exchange1.id)
    text = response.contents.first.text

    assert_match(/BTC/, text)
    assert_no_match(/ETH/, text)
  end

  test 'returns error for invalid exchange' do
    response = ExportTransactionsCsvTool.call('exchange_id' => 99_999)
    text = response.contents.first.text

    assert_match(/Exchange not found/, text)
  end
end
