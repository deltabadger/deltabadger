require 'test_helper'

class AccountTransactionSyncTest < ActiveSupport::TestCase
  setup do
    @user = create(:user)
    @exchange = create(:binance_exchange)
    @api_key = create(:api_key, user: @user, exchange: @exchange)

    @ledger_entries = [
      {
        entry_type: :buy,
        base_currency: 'BTC',
        base_amount: 0.5,
        quote_currency: 'USD',
        quote_amount: 25_000.0,
        fee_currency: 'USD',
        fee_amount: 25.0,
        tx_id: 'trade-1',
        group_id: nil,
        description: nil,
        transacted_at: Time.utc(2026, 3, 20, 10, 0, 0),
        raw_data: { 'orderId' => 'trade-1' }
      },
      {
        entry_type: :deposit,
        base_currency: 'USD',
        base_amount: 10_000.0,
        quote_currency: nil,
        quote_amount: nil,
        fee_currency: nil,
        fee_amount: nil,
        tx_id: 'deposit-1',
        group_id: nil,
        description: nil,
        transacted_at: Time.utc(2026, 3, 19, 8, 0, 0),
        raw_data: { 'txId' => 'deposit-1' }
      }
    ]
  end

  test 'imports ledger entries as account transactions' do
    @exchange.stubs(:get_ledger).returns(Result::Success.new(@ledger_entries))

    result = AccountTransactionSync.new(@api_key).sync!

    assert result.success?
    assert_equal 2, result.data
    assert_equal 2, AccountTransaction.count

    buy = AccountTransaction.find_by(tx_id: 'trade-1')
    assert buy.buy?
    assert_equal 'BTC', buy.base_currency
    assert_equal 0.5, buy.base_amount
    assert_equal 'USD', buy.quote_currency
    assert_equal 25_000.0, buy.quote_amount
    assert_equal 'USD', buy.fee_currency
    assert_equal 25.0, buy.fee_amount
    assert_equal @api_key, buy.api_key
    assert_equal @exchange, buy.exchange

    deposit = AccountTransaction.find_by(tx_id: 'deposit-1')
    assert deposit.deposit?
    assert_equal 'USD', deposit.base_currency
    assert_equal 10_000.0, deposit.base_amount
  end

  test 'skips duplicate entries by tx_id' do
    @exchange.stubs(:get_ledger).returns(Result::Success.new(@ledger_entries))

    AccountTransactionSync.new(@api_key).sync!
    result = AccountTransactionSync.new(@api_key).sync!

    assert result.success?
    assert_equal 0, result.data
    assert_equal 2, AccountTransaction.count
  end

  test 'updates last_synced_at on success' do
    @exchange.stubs(:get_ledger).returns(Result::Success.new([]))

    assert_nil @api_key.last_synced_at

    freeze_time do
      AccountTransactionSync.new(@api_key).sync!
      assert_equal Time.current, @api_key.reload.last_synced_at
    end
  end

  test 'passes start_time from last_synced_at' do
    last_sync = 1.day.ago
    @api_key.update!(last_synced_at: last_sync)

    @exchange.expects(:get_ledger).with(api_key: @api_key, start_time: last_sync).returns(Result::Success.new([]))

    AccountTransactionSync.new(@api_key).sync!
  end

  test 'returns failure when exchange returns failure' do
    @exchange.stubs(:get_ledger).returns(Result::Failure.new('API error'))

    result = AccountTransactionSync.new(@api_key).sync!

    assert result.failure?
    assert_equal 0, AccountTransaction.count
    assert_nil @api_key.reload.last_synced_at
  end

  test 'matches bot transactions by tx_id' do
    bot = create(:dca_single_asset, user: @user, exchange: @exchange, with_api_key: false)
    bot_tx = create(:transaction, bot: bot, exchange: @exchange, external_id: 'trade-1')

    @exchange.stubs(:get_ledger).returns(Result::Success.new(@ledger_entries))

    AccountTransactionSync.new(@api_key).sync!

    at = AccountTransaction.find_by(tx_id: 'trade-1')
    assert_equal bot_tx, at.bot_transaction
  end

  test 'does not match non-trade entries to bot transactions' do
    bot = create(:dca_single_asset, user: @user, exchange: @exchange, with_api_key: false)
    create(:transaction, bot: bot, exchange: @exchange, external_id: 'deposit-1')

    @exchange.stubs(:get_ledger).returns(Result::Success.new(@ledger_entries))

    AccountTransactionSync.new(@api_key).sync!

    deposit = AccountTransaction.find_by(tx_id: 'deposit-1')
    assert_nil deposit.bot_transaction
  end

  test 'handles entries without tx_id' do
    entries = [
      {
        entry_type: :staking_reward,
        base_currency: 'ETH',
        base_amount: 0.01,
        quote_currency: nil,
        quote_amount: nil,
        fee_currency: nil,
        fee_amount: nil,
        tx_id: nil,
        group_id: nil,
        description: 'Staking reward',
        transacted_at: Time.utc(2026, 3, 21),
        raw_data: {}
      }
    ]

    @exchange.stubs(:get_ledger).returns(Result::Success.new(entries))

    result = AccountTransactionSync.new(@api_key).sync!

    assert result.success?
    assert_equal 1, result.data
    reward = AccountTransaction.last
    assert reward.staking_reward?
    assert_nil reward.tx_id
  end
end
