require 'test_helper'

class Exchanges::CoinbaseGetLedgerTest < ActiveSupport::TestCase
  setup do
    @user = create(:user)
    @exchange = create(:coinbase_exchange)
    @api_key = create(:api_key, user: @user, exchange: @exchange)
  end

  test 'returns normalized trade entries from fills' do
    honeymaker_client = mock('honeymaker_client')
    Honeymaker.stubs(:client).returns(honeymaker_client)

    honeymaker_client.stubs(:list_fills).returns(
      Result::Success.new({
                            'fills' => [
                              {
                                'trade_id' => 't1',
                                'product_id' => 'BTC-USD',
                                'side' => 'BUY',
                                'price' => '50000',
                                'size' => '0.5',
                                'commission' => '25',
                                'trade_time' => '2026-03-20T10:00:00Z'
                              }
                            ],
                            'cursor' => ''
                          })
    )
    honeymaker_client.stubs(:list_accounts).returns(Result::Success.new({ 'accounts' => [] }))

    result = @exchange.get_ledger(api_key: @api_key)

    assert result.success?
    trade = result.data.find { |e| e[:entry_type] == :buy }
    assert_not_nil trade
    assert_equal 'BTC', trade[:base_currency]
    assert_equal 0.5, trade[:base_amount]
    assert_equal 'USD', trade[:quote_currency]
    assert_equal 25.0, trade[:fee_amount]
  end

  test 'returns deposit entries from account transactions' do
    honeymaker_client = mock('honeymaker_client')
    Honeymaker.stubs(:client).returns(honeymaker_client)

    honeymaker_client.stubs(:list_fills).returns(Result::Success.new({ 'fills' => [], 'cursor' => '' }))
    honeymaker_client.stubs(:list_accounts).returns(
      Result::Success.new({ 'accounts' => [{ 'uuid' => 'acc1' }] })
    )
    honeymaker_client.stubs(:list_transactions).returns(
      Result::Success.new({
                            'data' => [
                              {
                                'id' => 'tx1',
                                'type' => 'receive',
                                'amount' => { 'amount' => '1.0', 'currency' => 'BTC' },
                                'network' => { 'transaction_fee' => { 'amount' => '0.0001', 'currency' => 'BTC' } },
                                'created_at' => '2026-03-19T08:00:00Z',
                                'description' => 'Received Bitcoin'
                              }
                            ],
                            'pagination' => { 'next_starting_after' => nil }
                          })
    )

    result = @exchange.get_ledger(api_key: @api_key)

    assert result.success?
    deposit = result.data.find { |e| e[:entry_type] == :deposit }
    assert_not_nil deposit
    assert_equal 'BTC', deposit[:base_currency]
    assert_equal 1.0, deposit[:base_amount]
    assert_equal 'Received Bitcoin', deposit[:description]
  end

  test 'detects crypto-to-crypto swaps' do
    honeymaker_client = mock('honeymaker_client')
    Honeymaker.stubs(:client).returns(honeymaker_client)

    honeymaker_client.stubs(:list_fills).returns(
      Result::Success.new({
                            'fills' => [
                              {
                                'trade_id' => 't2',
                                'product_id' => 'ETH-BTC',
                                'side' => 'BUY',
                                'price' => '0.05',
                                'size' => '10.0',
                                'commission' => '0.001',
                                'trade_time' => '2026-03-20T10:00:00Z'
                              }
                            ],
                            'cursor' => ''
                          })
    )
    honeymaker_client.stubs(:list_accounts).returns(Result::Success.new({ 'accounts' => [] }))

    result = @exchange.get_ledger(api_key: @api_key)

    assert result.success?
    swap_entries = result.data.select { |e| e[:group_id].present? }
    assert_equal 2, swap_entries.size
    assert_equal swap_entries[0][:group_id], swap_entries[1][:group_id]
  end

  test 'returns failure on API error' do
    honeymaker_client = mock('honeymaker_client')
    Honeymaker.stubs(:client).returns(honeymaker_client)

    honeymaker_client.stubs(:list_fills).returns(Result::Failure.new('Unauthorized'))

    result = @exchange.get_ledger(api_key: @api_key)
    assert result.failure?
  end
end
