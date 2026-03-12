require 'test_helper'

class ListTransactionsToolTest < ActiveSupport::TestCase
  setup do
    @user = create(:user, admin: true)
    ActionMCP::Current.stubs(:user).returns(@user)
  end

  test 'lists recent transactions' do
    bot = create(:dca_single_asset, user: @user)
    create(:transaction, bot: bot, status: :submitted, amount_exec: 0.001, price: 50_000, quote_amount_exec: 50)

    response = ListTransactionsTool.call
    text = response.contents.first.text

    assert_match(/Transactions \(1\)/, text)
    assert_match(/BUY/, text)
    assert_match(/0.001/, text)
    assert_match(/50000/, text)
  end

  test 'filters by bot_id' do
    btc = create(:asset, :bitcoin)
    eth = create(:asset, :ethereum)
    usd = create(:asset, :usd)
    exchange = create(:binance_exchange)
    bot1 = create(:dca_single_asset, user: @user, base_asset: btc, quote_asset: usd, exchange: exchange)
    bot2 = create(:dca_single_asset, user: @user, base_asset: eth, quote_asset: usd, exchange: exchange)
    create(:transaction, bot: bot1)
    create(:transaction, bot: bot2)

    response = ListTransactionsTool.call('bot_id' => bot1.id)
    text = response.contents.first.text

    assert_match(/Transactions \(1\)/, text)
  end

  test 'respects limit parameter' do
    bot = create(:dca_single_asset, user: @user)
    3.times { create(:transaction, bot: bot) }

    response = ListTransactionsTool.call('limit' => 2)
    text = response.contents.first.text

    assert_match(/Transactions \(2\)/, text)
  end

  test 'returns empty message when no transactions' do
    response = ListTransactionsTool.call
    text = response.contents.first.text

    assert_equal 'No transactions found.', text
  end

  test 'returns bot not found for invalid bot_id' do
    response = ListTransactionsTool.call('bot_id' => 99_999)
    text = response.contents.first.text

    assert_equal 'Bot not found.', text
  end
end
