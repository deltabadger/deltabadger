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

  test 'trade fee is reported separately and base_amount stays gross' do
    honeymaker_client = mock('honeymaker_client')
    Honeymaker.stubs(:client).returns(honeymaker_client)

    honeymaker_client.stubs(:list_fills).returns(
      Result::Success.new({
                            'fills' => [
                              {
                                'trade_id' => 't-gross',
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
    trade = result.data.find { |entry| entry[:entry_type] == :buy }
    assert_not_nil trade
    # The tax engine capitalises acquisition fees and would double-count if base_amount ever became net.
    assert_equal 0.5.to_d, trade[:base_amount]
    assert_equal 'USD', trade[:fee_currency]
    assert_equal 25.to_d, trade[:fee_amount]
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

  test 'reads every page of accounts, 250 at a time' do
    honeymaker_client = mock('honeymaker_client')
    Honeymaker.stubs(:client).returns(honeymaker_client)
    honeymaker_client.stubs(:list_fills).returns(Result::Success.new({ 'fills' => [], 'cursor' => '' }))
    honeymaker_client.expects(:list_accounts).with(limit: 250, cursor: nil)
                     .returns(Result::Success.new({ 'accounts' => [{ 'uuid' => 'acc1' }], 'has_next' => true, 'cursor' => 'c2' }))
    honeymaker_client.expects(:list_accounts).with(limit: 250, cursor: 'c2')
                     .returns(Result::Success.new({ 'accounts' => [{ 'uuid' => 'acc2' }], 'has_next' => false, 'cursor' => '' }))
    honeymaker_client.stubs(:list_transactions).returns(Result::Success.new({ 'data' => [] }))
    honeymaker_client.expects(:list_transactions).with(account_id: 'acc2', limit: 100, starting_after: nil)
                     .returns(Result::Success.new({ 'data' => [] }))

    assert @exchange.get_ledger(api_key: @api_key).success?
  end

  test 'a failed transactions page fails the ledger' do
    honeymaker_client = mock('honeymaker_client')
    Honeymaker.stubs(:client).returns(honeymaker_client)
    honeymaker_client.stubs(:list_fills).returns(Result::Success.new({ 'fills' => [], 'cursor' => '' }))
    honeymaker_client.stubs(:list_accounts).returns(Result::Success.new({ 'accounts' => [{ 'uuid' => 'acc1' }] }))
    honeymaker_client.stubs(:list_transactions).returns(Result::Failure.new('Internal Server Error'))

    assert @exchange.get_ledger(api_key: @api_key).failure?
  end

  test 'an adjusted fill keeps its own tx_id instead of the trade_id it shares' do
    honeymaker_client = mock('honeymaker_client')
    Honeymaker.stubs(:client).returns(honeymaker_client)
    fill = { 'trade_id' => 't1', 'product_id' => 'BTC-USD', 'side' => 'BUY', 'price' => '50000', 'size' => '0.5',
             'commission' => '25', 'trade_time' => '2026-03-20T10:00:00Z' }
    honeymaker_client.stubs(:list_fills).returns(
      Result::Success.new({ 'fills' => [fill.merge('entry_id' => 'e1', 'trade_type' => 'FILL'),
                                        fill.merge('entry_id' => 'e2', 'trade_type' => 'CORRECTION')],
                            'cursor' => '' })
    )
    honeymaker_client.stubs(:list_accounts).returns(Result::Success.new({ 'accounts' => [] }))

    assert_equal %w[t1 e2], @exchange.get_ledger(api_key: @api_key).data.pluck(:tx_id)
  end

  # Before entry_id, an adjusted fill was stored under the trade_id it shares with its FILL, and the
  # FILL itself was then dropped as that row's duplicate. A re-read rekeys the stored row to its
  # entry_id instead of storing it twice, and the FILL it hid is imported.
  test 'an adjusted fill stored under its trade_id is rekeyed, not imported again' do
    fill = { 'trade_id' => 't1', 'product_id' => 'BTC-USD', 'side' => 'BUY', 'price' => '50000',
             'commission' => '0', 'trade_time' => '2026-03-20T10:00:00Z' }
    regular = fill.merge('entry_id' => 'e1', 'trade_type' => 'FILL', 'size' => '0.5')
    adjusted = fill.merge('entry_id' => 'e2', 'trade_type' => 'CORRECTION', 'size' => '0.1')
    old = create(:account_transaction, api_key: @api_key, tx_id: 't1', base_amount: 0.1, quote_amount: 5000,
                                       transacted_at: Time.utc(2026, 3, 20, 10), raw_data: adjusted)
    honeymaker_client = mock('honeymaker_client')
    Honeymaker.stubs(:client).returns(honeymaker_client)
    honeymaker_client.stubs(:list_fills).returns(Result::Success.new({ 'fills' => [regular, adjusted], 'cursor' => '' }))
    honeymaker_client.stubs(:list_accounts).returns(Result::Success.new({ 'accounts' => [] }))

    AccountTransactionSync.new(@api_key).sync!

    assert_equal 'e2', old.reload.tx_id
    assert_equal %w[e2 t1], AccountTransaction.where(api_key: @api_key).order(:tx_id).pluck(:tx_id)
  end

  test 'an adjusted fill stored without an entry_id is rekeyed on the same event' do
    fill = { 'trade_id' => 't1', 'product_id' => 'BTC-USD', 'side' => 'BUY', 'price' => '50000', 'size' => '0.1',
             'commission' => '0', 'trade_time' => '2026-03-20T10:00:00Z' }
    old = create(:account_transaction, api_key: @api_key, tx_id: 't1', base_amount: 0.1, quote_amount: 5000,
                                       transacted_at: Time.utc(2026, 3, 20, 10), raw_data: fill)
    honeymaker_client = mock('honeymaker_client')
    Honeymaker.stubs(:client).returns(honeymaker_client)
    honeymaker_client.stubs(:list_fills).returns(
      Result::Success.new({ 'fills' => [fill.merge('entry_id' => 'e2', 'trade_type' => 'CORRECTION')], 'cursor' => '' })
    )
    honeymaker_client.stubs(:list_accounts).returns(Result::Success.new({ 'accounts' => [] }))

    AccountTransactionSync.new(@api_key).sync!

    assert_equal ['e2'], AccountTransaction.where(api_key: @api_key).pluck(:tx_id)
    assert_equal old.id, AccountTransaction.find_by(tx_id: 'e2').id
  end
end
