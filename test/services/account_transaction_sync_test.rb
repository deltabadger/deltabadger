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

  test 'empty ledger keeps a nil last_synced_at' do
    @exchange.stubs(:get_ledger).returns(Result::Success.new([]))

    assert_nil @api_key.last_synced_at

    AccountTransactionSync.new(@api_key).sync!

    assert_nil @api_key.reload.last_synced_at
  end

  test 'empty ledger keeps the existing last_synced_at' do
    previous_watermark = Time.utc(2026, 3, 18, 7, 30, 0)
    @api_key.update!(last_synced_at: previous_watermark)
    @exchange.stubs(:get_ledger).returns(Result::Success.new([]))

    AccountTransactionSync.new(@api_key).sync!

    assert_equal previous_watermark, @api_key.reload.last_synced_at
  end

  test 'sets last_synced_at to the latest fetched transaction time' do
    @exchange.stubs(:get_ledger).returns(Result::Success.new(@ledger_entries))

    AccountTransactionSync.new(@api_key).sync!

    assert_equal Time.utc(2026, 3, 20, 10, 0, 0), @api_key.reload.last_synced_at
  end

  test 'passes start_time from last_synced_at' do
    last_sync = 2.days.ago.change(usec: 0)
    @api_key.update!(last_synced_at: last_sync)

    @exchange.expects(:get_ledger)
             .with(api_key: @api_key, start_time: last_sync - 25.hours)
             .returns(Result::Success.new([]))

    AccountTransactionSync.new(@api_key).sync!
  end

  # Binance, Bybit, MEXC, KuCoin and Bitget cap a history query at ~90 days FROM start_time. Without
  # a floor, a quiet account's data-derived watermark stops reaching the present and goes blind.
  test 'floors start_time so a long-quiet account still queries up to the present' do
    @api_key.update!(last_synced_at: 2.years.ago)
    captured = nil
    @exchange.stubs(:get_ledger).with do |args|
      captured = args[:start_time]
      true
    end.returns(Result::Success.new([]))

    AccountTransactionSync.new(@api_key).sync!

    assert_operator captured, :>, 81.days.ago
    assert_operator captured, :<, 79.days.ago
  end

  test 'passes nil start_time when the watermark is nil even if transactions already exist' do
    create(:account_transaction, api_key: @api_key, exchange: @exchange,
                                 transacted_at: Time.utc(2026, 3, 10, 9, 0, 0))
    assert_nil @api_key.last_synced_at
    @exchange.expects(:get_ledger)
             .with(api_key: @api_key, start_time: nil)
             .returns(Result::Success.new([]))

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

  test 'imports the same tx_id once for each user on a shared exchange account' do
    second_user = create(:user)
    second_api_key = create(:api_key, user: second_user, exchange: @exchange)
    @exchange.stubs(:get_ledger).returns(Result::Success.new(@ledger_entries))

    first_result = AccountTransactionSync.new(@api_key).sync!
    second_result = AccountTransactionSync.new(second_api_key).sync!

    assert_equal 2, first_result.data
    assert_equal 2, second_result.data
    assert_equal 4, AccountTransaction.count
    assert_equal [@user.id, second_user.id].sort,
                 AccountTransaction.where(tx_id: 'trade-1').pluck(:user_id).sort
  end

  test 'nil tx_id dedup uses the complete fallback identity' do
    transacted_at = Time.utc(2026, 3, 21, 14, 0, 0)
    entry = {
      entry_type: :staking_reward,
      base_currency: 'ETH',
      base_amount: '0.0125'.to_d,
      quote_currency: nil,
      quote_amount: nil,
      fee_currency: nil,
      fee_amount: nil,
      tx_id: nil,
      group_id: nil,
      description: 'Invented staking reward',
      transacted_at: transacted_at,
      raw_data: { 'source' => 'invented-ledger' }
    }

    other_user = create(:user)
    other_user_key = create(:api_key, user: other_user, exchange: @exchange)
    create(:account_transaction, api_key: other_user_key, exchange: @exchange,
                                 entry_type: entry[:entry_type], base_currency: entry[:base_currency],
                                 base_amount: entry[:base_amount], tx_id: nil, transacted_at: transacted_at)

    other_exchange = create(:kraken_exchange)
    other_exchange_key = create(:api_key, user: @user, exchange: other_exchange)
    create(:account_transaction, api_key: other_exchange_key, exchange: other_exchange,
                                 entry_type: entry[:entry_type], base_currency: entry[:base_currency],
                                 base_amount: entry[:base_amount], tx_id: nil, transacted_at: transacted_at)

    @exchange.stubs(:get_ledger).returns(Result::Success.new([entry]))
    first_result = AccountTransactionSync.new(@api_key).sync!

    entries = [
      entry.merge(description: 'Changed description is not part of the identity',
                  raw_data: { 'source' => 'changed-raw-data' }),
      entry.merge(entry_type: :other_income),
      entry.merge(base_currency: 'SOL'),
      entry.merge(base_amount: '0.0126'.to_d),
      entry.merge(transacted_at: transacted_at + 1.second)
    ]
    @exchange.stubs(:get_ledger).returns(Result::Success.new(entries))
    second_result = AccountTransactionSync.new(@api_key).sync!

    assert_equal 1, first_result.data
    assert_equal 4, second_result.data
    assert_equal 5, AccountTransaction.where(user: @user, exchange: @exchange).count
  end

  # Bybit returns an empty txID for internal transfers, and Gemini/Hyperliquid/BingX build their
  # tx_id with .to_s on a field that can be missing. A blank id identifies nothing, so it must fall
  # through to the nil-tx_id identity rather than collapsing every such row onto one '' key.
  test 'treats a blank tx_id as no tx_id' do
    base = {
      entry_type: :deposit,
      base_currency: 'USDT',
      quote_currency: nil,
      quote_amount: nil,
      fee_currency: nil,
      fee_amount: nil,
      tx_id: '',
      group_id: nil,
      description: 'Internal transfer',
      transacted_at: Time.utc(2026, 4, 2, 9, 0, 0),
      raw_data: { 'txID' => '' }
    }
    entries = [base.merge(base_amount: '125.5'.to_d), base.merge(base_amount: '310.25'.to_d)]
    @exchange.stubs(:get_ledger).returns(Result::Success.new(entries))

    first_result = AccountTransactionSync.new(@api_key).sync!
    second_result = AccountTransactionSync.new(@api_key).sync!

    assert_equal 2, first_result.data
    assert_equal 0, second_result.data
    assert_equal 2, AccountTransaction.count
    assert_equal [nil, nil], AccountTransaction.pluck(:tx_id)
  end

  test 'skips a standalone split leg after its merged adjustment was imported' do
    transacted_at = Time.utc(2026, 5, 1)
    merged_entry = {
      entry_type: :adjustment,
      base_currency: 'ZZTOP',
      base_amount: 20.to_d,
      quote_currency: nil,
      quote_amount: nil,
      fee_currency: nil,
      fee_amount: nil,
      tx_id: 'split-remove-1',
      group_id: nil,
      description: 'Split (ZZTOP)',
      transacted_at: transacted_at,
      raw_data: {
        'id' => 'split-remove-1',
        'activity_type' => 'SPLIT',
        'merged_activity_ids' => %w[split-remove-1 split-add-1]
      }
    }
    standalone_entry = merged_entry.merge(
      base_amount: 30.to_d,
      tx_id: 'split-add-1',
      raw_data: { 'id' => 'split-add-1', 'activity_type' => 'SPLIT' }
    )
    @exchange.stubs(:get_ledger).returns(Result::Success.new([merged_entry]))
    AccountTransactionSync.new(@api_key).sync!
    @exchange.stubs(:get_ledger).returns(Result::Success.new([standalone_entry]))

    result = AccountTransactionSync.new(@api_key).sync!

    assert_equal 0, result.data
    assert_equal ['split-remove-1'], AccountTransaction.pluck(:tx_id)
  end

  # The deliberate asymmetry: only the STORED side is expanded to merged legs. Skipping the merged
  # row here would leave the ledger permanently one-legged (-20 where the truth is +10-20), a wrong
  # share count that reads as data. A visible double count is the lesser, correctable error.
  test 'still imports a merged adjustment when one of its legs was already imported standalone' do
    transacted_at = Time.utc(2026, 5, 2)
    standalone_entry = {
      entry_type: :adjustment,
      base_currency: 'QQTEST',
      base_amount: 10.to_d,
      quote_currency: nil,
      quote_amount: nil,
      fee_currency: nil,
      fee_amount: nil,
      tx_id: 'split-add-2',
      group_id: nil,
      description: 'Split (QQTEST)',
      transacted_at: transacted_at,
      raw_data: { 'id' => 'split-add-2', 'activity_type' => 'SPLIT' }
    }
    merged_entry = standalone_entry.merge(
      base_amount: -20.to_d,
      tx_id: 'split-remove-2',
      raw_data: {
        'id' => 'split-remove-2',
        'activity_type' => 'SPLIT',
        'merged_activity_ids' => %w[split-remove-2 split-add-2]
      }
    )
    @exchange.stubs(:get_ledger).returns(Result::Success.new([standalone_entry]))
    AccountTransactionSync.new(@api_key).sync!
    @exchange.stubs(:get_ledger).returns(Result::Success.new([merged_entry]))

    result = AccountTransactionSync.new(@api_key).sync!

    assert_equal 1, result.data
    assert_equal %w[split-add-2 split-remove-2], AccountTransaction.pluck(:tx_id).sort
  end

  test 'logs an error and continues past unsavable Alpaca activities, holding the watermark back' do
    alpaca = create(:alpaca_exchange)
    alpaca_key = create(:api_key, user: @user, exchange: alpaca, passphrase: 'live')
    activities = [
      {
        'id' => 'split-missing-symbol', 'activity_type' => 'SPLIT',
        'qty' => '5', 'net_amount' => '0', 'date' => '2026-05-16', 'status' => 'executed'
      },
      {
        'id' => 'roc-missing-symbol', 'activity_type' => 'DIVROC',
        'qty' => '2', 'net_amount' => '7.25', 'date' => '2026-05-17', 'status' => 'executed'
      },
      {
        'id' => 'interest-missing-time', 'activity_type' => 'INT',
        'net_amount' => '1.15', 'status' => 'executed'
      },
      {
        'id' => 'good-interest', 'activity_type' => 'INT',
        'net_amount' => '2.35', 'date' => '2026-05-18', 'status' => 'executed'
      }
    ]
    client = mock('alpaca_client')
    client.stubs(:get_account_activities).returns(Result::Success.new(activities))
    alpaca.stubs(:client).returns(client)
    Rails.logger.expects(:error).at_least(3)

    result = AccountTransactionSync.new(alpaca_key).sync!

    assert_instance_of Result::Success, result
    assert_equal 1, result.data
    assert_equal ['good-interest'], AccountTransaction.where(api_key: alpaca_key).pluck(:tx_id)
    # Clamped to the earliest skipped row, not the newest fetched one: advancing to 05-18 would put
    # the failed 05-16 entry outside every future window, making one bad row a permanent hole.
    assert_equal Time.utc(2026, 5, 16), alpaca_key.reload.last_synced_at
  end

  test 'the nil tx_id fallback does not swallow rows from another key on the same exchange' do
    # ApiKey allows one key per (user, exchange, key_type), so a second key on the same exchange is
    # reachable today; the callers pass arbitrary key ids to the sync, not just trading ones.
    second_key = create(:api_key, user: @user, exchange: @exchange, key_type: :withdrawal)
    entry = {
      entry_type: :staking_reward,
      base_currency: 'ETH',
      base_amount: '0.02'.to_d,
      quote_currency: nil,
      quote_amount: nil,
      fee_currency: nil,
      fee_amount: nil,
      tx_id: nil,
      group_id: nil,
      description: 'Staking reward',
      transacted_at: Time.utc(2026, 4, 9, 6, 0, 0),
      raw_data: {}
    }
    @exchange.stubs(:get_ledger).returns(Result::Success.new([entry]))

    first_result = AccountTransactionSync.new(@api_key).sync!
    second_result = AccountTransactionSync.new(second_key).sync!

    assert_equal 1, first_result.data
    assert_equal 1, second_result.data
    assert_equal [@api_key.id, second_key.id].sort,
                 AccountTransaction.where(tx_id: nil).pluck(:api_key_id).sort
  end
end
