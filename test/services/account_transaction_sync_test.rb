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

  # A NaN amount binds as NULL and raises at its insert; the rows saved before it would stay while
  # the watermark does not move. The sync fails before it writes anything instead (as the Rust port).
  test 'an amount that is not a finite number fails the sync before any row is written' do
    watermark = Time.utc(2026, 3, 1)
    @api_key.update!(last_synced_at: watermark)
    [BigDecimal('NaN'), BigDecimal('Infinity'), Float::NAN].each do |amount|
      split = @ledger_entries.first.merge(entry_type: :adjustment, base_currency: 'AAPL', base_amount: amount,
                                          quote_currency: nil, quote_amount: nil, fee_currency: nil, fee_amount: nil,
                                          tx_id: 'split-1', transacted_at: Time.utc(2026, 3, 21))
      @api_key.exchange.stubs(:get_ledger).returns(Result::Success.new(@ledger_entries + [split]))

      assert_raises(ArgumentError) { AccountTransactionSync.new(@api_key).sync! }
      assert_equal 0, AccountTransaction.count
      assert_equal watermark, @api_key.reload.last_synced_at
    end
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

  test 'a completed run clears the stored sync error' do
    @api_key.update_column(:last_sync_error, 'StandardError: API error')
    @exchange.stubs(:get_ledger).returns(Result::Success.new(@ledger_entries))

    AccountTransactionSync.new(@api_key).sync!

    assert_nil @api_key.reload.last_sync_error
  end

  test 'a failed fetch leaves the stored sync error for the caller to write' do
    @api_key.update_column(:last_sync_error, 'StandardError: API error')
    @exchange.stubs(:get_ledger).returns(Result::Failure.new('API down'))

    AccountTransactionSync.new(@api_key).sync!

    assert_equal 'StandardError: API error', @api_key.reload.last_sync_error
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

  # A row dated ahead of today (a split announced before its date) must not carry the watermark into the
  # future: every fill, dividend and deposit between now and that date would fall outside every later window.
  test 'a future-dated row does not move the watermark past the sync start' do
    travel_to Time.utc(2026, 9, 20, 2, 0, 0) do
      split = @ledger_entries.first.merge(entry_type: :adjustment, base_currency: 'KLAC', base_amount: 90,
                                          quote_currency: nil, quote_amount: nil, fee_amount: nil,
                                          tx_id: 'split-1', transacted_at: Time.utc(2026, 10, 5), raw_data: {})
      @exchange.stubs(:get_ledger).returns(Result::Success.new(@ledger_entries + [split]))
      AccountTransactionSync.new(@api_key).sync!

      assert_equal Time.utc(2026, 9, 20, 2, 0, 0), ApiKey.find(@api_key.id).last_synced_at
    end

    travel_to Time.utc(2026, 9, 21, 2, 0, 0) do
      @exchange.expects(:get_ledger)
               .with(api_key: @api_key, start_time: Time.utc(2026, 9, 19, 1, 0, 0))
               .returns(Result::Success.new([]))
      AccountTransactionSync.new(@api_key).sync!
    end
  end

  # A key left behind by the old rule holds a watermark ahead of today: it is read as now, and the next
  # completed sync pulls it back.
  test 'a stored watermark ahead of now is read as now' do
    travel_to Time.utc(2026, 9, 21, 2, 0, 0) do
      @api_key.update!(last_synced_at: Time.utc(2026, 10, 5))
      @exchange.expects(:get_ledger)
               .with(api_key: @api_key, start_time: Time.utc(2026, 9, 20, 1, 0, 0))
               .returns(Result::Success.new([]))
      AccountTransactionSync.new(@api_key).sync!

      assert_equal Time.utc(2026, 9, 21, 2, 0, 0), @api_key.reload.last_synced_at
    end
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

  # Alpaca, Kraken, Coinbase and Hyperliquid all paginate a cursor from start_time to the present,
  # and nothing syncs on a schedule — a sync only happens when the user opens the tracker. Flooring
  # an uncapped venue would silently drop every month between two visits, and the watermark would
  # then advance past them.
  test 'an uncapped exchange is never clamped, however old the watermark' do
    exchange = create(:kraken_exchange)
    last_sync = 200.days.ago.change(usec: 0)
    api_key = create(:api_key, user: @user, exchange: exchange, last_synced_at: last_sync)

    assert_nil exchange.ledger_window
    exchange.expects(:get_ledger)
            .with(api_key: api_key, start_time: last_sync - 25.hours)
            .returns(Result::Success.new([]))

    AccountTransactionSync.new(api_key).sync!
  end

  # Bybit's execution list and KuCoin's fills serve 7 days measured FROM startTime, so an 80-day-old
  # start returns a window that ended 73 days ago: a deposit made today is never seen, and nothing
  # new can arrive to advance the watermark past it.
  test 'floors start_time at the exchange own ledger window, not the 80-day default' do
    exchange = create(:bybit_exchange)
    api_key = create(:api_key, user: @user, exchange: exchange, last_synced_at: 2.years.ago)
    captured = nil
    exchange.stubs(:get_ledger).with do |args|
      captured = args[:start_time]
      true
    end.returns(Result::Success.new([]))

    AccountTransactionSync.new(api_key).sync!

    assert_operator captured, :>, 8.days.ago
    assert_operator captured, :<, 6.days.ago
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

  # A broker whose ledger is keyed by FILL is the normal case, not the exception: Alpaca's activity
  # id (`20260821095224541::82693d41-…`) is not the order id the bot placed, so matching on tx_id
  # alone left every single row unlinked. One order can fill many times, and each fill points at
  # the order that made it.
  test 'matches a bot order through the fill it produced' do
    bot = create(:dca_single_asset, user: @user, exchange: @exchange, with_api_key: false)
    bot_tx = create(:transaction, bot: bot, exchange: @exchange, external_id: 'order-7')
    fills = [@ledger_entries.first.merge(tx_id: 'fill-a', raw_data: { 'order_id' => 'order-7' }),
             @ledger_entries.first.merge(tx_id: 'fill-b', raw_data: { 'order_id' => 'order-7' })]
    @exchange.stubs(:get_ledger).returns(Result::Success.new(fills))

    AccountTransactionSync.new(@api_key).sync!

    linked = %w[fill-a fill-b].map { |id| AccountTransaction.find_by(tx_id: id).bot_transaction }
    assert_equal [bot_tx, bot_tx], linked
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

  # Binance's export writes whole seconds and ROUNDS: the Convert its API stamps at 06:22:53.911
  # is 06:22:54 in the file. Bucketed by second those are two events, and the coins arrive twice.
  # A row with no id is the same event as a stored one within a second of it, either way round;
  # a full second later is still a row of its own.
  test 'a file row on the rounded-up second is the stored API row' do
    at = Time.utc(2021, 9, 20, 6, 22, 53, 911_000)
    api = { entry_type: :swap_in, base_currency: 'LTC', base_amount: '0.89568816'.to_d,
            quote_currency: nil, quote_amount: nil, fee_currency: nil, fee_amount: nil,
            tx_id: 'quote-1-in', group_id: 'convert_quote-1', description: 'Convert',
            transacted_at: at, raw_data: {} }
    @exchange.stubs(:get_ledger).returns(Result::Success.new([api]))
    AccountTransactionSync.new(@api_key).sync!

    file = api.merge(tx_id: nil, group_id: 'swapcsv_1', description: nil, transacted_at: Time.utc(2021, 9, 20, 6, 22, 54))
    rounded = AccountTransactionSync.new(@api_key).store!([file])
    later = AccountTransactionSync.new(@api_key).store!([file.merge(transacted_at: at + 1.001)])

    assert_equal({ imported: 0, duplicates: 1 }, rounded.slice(:imported, :duplicates), 'the same Convert')
    assert_equal({ imported: 1, duplicates: 0 }, later.slice(:imported, :duplicates), 'over a second on: its own row')
  end

  # The API cuts a Convert to the whole second, the export rounds it up: 14:29:18 against 14:29:19.
  test 'a file row exactly a second after the API row is the same event, but not after a file row' do
    at = Time.utc(2021, 3, 10, 14, 29, 18)
    api = { entry_type: :swap_out, base_currency: 'USDT', base_amount: '12.90748981'.to_d,
            tx_id: '11070031571-out', transacted_at: at, raw_data: {} }
    file = api.merge(tx_id: nil, transacted_at: at + 1.second)

    assert_equal 1, AccountTransactionSync.new(@api_key).store!([api])[:imported]
    assert_equal 0, AccountTransactionSync.new(@api_key).store!([file])[:imported], 'the API row, a second off'

    AccountTransaction.delete_all
    AccountTransactionSync.new(@api_key).store!([file.merge(transacted_at: at)])
    assert_equal 1, AccountTransactionSync.new(@api_key).store!([file])[:imported], 'two file rows a second apart are two'
  end

  # An export repeats identical rows in one second, each a real credit. A row this run stored is no
  # evidence against the next; rows stored before it are, one for one.
  test 'identical rows in one batch are all stored, and only once' do
    row = { entry_type: :other_income, base_currency: 'USDT', base_amount: '0.06303219'.to_d, tx_id: nil,
            transacted_at: Time.utc(2021, 1, 7, 18, 28, 41), raw_data: { 'operation' => 'Commission History' } }
    store = -> { AccountTransactionSync.new(@api_key).store!([row, row.dup, row.dup]).slice(:imported, :duplicates) }

    assert_equal({ imported: 3, duplicates: 0 }, store.call)
    assert_equal({ imported: 0, duplicates: 3 }, store.call, 'the same batch again')

    AccountTransaction.order(:id).last.destroy!
    assert_equal({ imported: 1, duplicates: 2 }, store.call, 'only the surplus over what was stored')
    assert_equal 3, AccountTransaction.count
  end

  # A deposit whose id is already on a file row dated when it was credited; the id-less copy of the same deposit is
  # stored too. The id'd entry is skipped on its id, and must not use up the id-less one's match.
  test 'a row skipped on its id is not one of the identical rows counted' do
    at = Time.utc(2021, 7, 1, 11, 6, 41)
    row = { entry_type: :deposit, base_currency: 'XRP', base_amount: '30'.to_d, transacted_at: at, raw_data: {} }
    create(:account_transaction, api_key: @api_key, exchange: @exchange, **row, tx_id: 'dep-a', transacted_at: at + 46)
    create(:account_transaction, api_key: @api_key, exchange: @exchange, **row, tx_id: nil)

    counts = AccountTransactionSync.new(@api_key).store!([row.merge(tx_id: 'dep-a'), row.merge(tx_id: nil)])

    assert_equal({ imported: 0, duplicates: 2 }, counts.slice(:imported, :duplicates))
  end

  # The other way round: a file imported BEFORE the first sync stores its rows with no id, and the
  # API's copies then arrive with one. An id that matches nothing stored is not yet a new row — the
  # id-less row within a second of it, same type, coin and amount, is the same event.
  test 'an API row is the id-less file row already stored' do
    at = Time.utc(2021, 9, 20, 10, 10, 52)
    file = { entry_type: :withdrawal, base_currency: 'LTC', base_amount: '0.249'.to_d,
             quote_currency: nil, quote_amount: nil, fee_currency: nil, fee_amount: nil,
             tx_id: nil, group_id: nil, description: 'Withdraw', transacted_at: at, raw_data: {} }
    AccountTransactionSync.new(@api_key).store!([file])

    api = file.merge(tx_id: 'abc123', description: nil, transacted_at: at + 0.4, raw_data: { 'txId' => 'abc123' })
    counts = AccountTransactionSync.new(@api_key).store!([api, api.merge(tx_id: 'def456', transacted_at: at + 2.seconds)])

    assert_equal({ imported: 1, duplicates: 1 }, counts.slice(:imported, :duplicates))
  end

  # ---- Binance's export and its API on one transfer -------------------------------------------------
  # The export dates a deposit when it was credited, seconds to minutes after the API's insertTime, and
  # writes a withdrawal GROSS (amount plus network fee) at the moment it completed, where the API row is
  # net at the moment it was requested. Neither is within a second of the other, so each pair is matched
  # on its own terms: a deposit credited up to ten minutes after it was seen, a withdrawal whose gross is
  # the file's amount at its completeTime. One file row pairs with one API row, never more.

  def api_deposit(at:, amount: '150.2', id: 'dep-1', coin: 'USDT')
    { entry_type: :deposit, base_currency: coin, base_amount: amount.to_d, quote_currency: nil, quote_amount: nil,
      fee_currency: nil, fee_amount: nil, tx_id: id, group_id: nil, description: nil, transacted_at: at,
      raw_data: { 'amount' => amount, 'coin' => coin, 'status' => 1, 'txId' => id, 'insertTime' => (at.to_f * 1000).to_i } }
  end

  def api_withdrawal(apply:, complete:, amount: '10.43980476', fee: '0.001', id: 'wd-1')
    raw = { 'amount' => amount, 'transactionFee' => fee, 'coin' => 'LTC', 'status' => 6, 'txId' => id,
            'applyTime' => apply.strftime('%F %T') }
    raw['completeTime'] = complete.is_a?(Time) ? complete.strftime('%F %T') : complete unless complete == :none
    { entry_type: :withdrawal, base_currency: 'LTC', base_amount: amount.to_d, quote_currency: nil, quote_amount: nil,
      fee_currency: 'LTC', fee_amount: fee&.to_d, tx_id: id, group_id: nil, description: nil, transacted_at: apply,
      raw_data: raw }
  end

  def file_transfer(type, amount, at, coin: type == :deposit ? 'USDT' : 'LTC')
    operation = type == :deposit ? 'Deposit' : 'Withdraw'
    { entry_type: type, base_currency: coin, base_amount: amount.to_d, quote_currency: nil, quote_amount: nil,
      fee_currency: nil, fee_amount: nil, tx_id: nil, group_id: nil, description: operation, transacted_at: at,
      raw_data: { 'operation' => operation } }
  end

  def store(*entries) = AccountTransactionSync.new(@api_key).store!(entries)

  test 'an API deposit pairs with the file row credited up to ten minutes later, and claims it' do
    credited = Time.utc(2021, 7, 1, 11, 7, 4)
    { -46 => 1, -((9 * 60) + 59) => 1, -10 * 60 => 1, -601 => 2, -0.5 => 1, 0.5 => 2, 10 => 2 }.each do |offset, rows|
      AccountTransaction.delete_all
      store(file_transfer(:deposit, '150.2', credited))
      store(api_deposit(at: credited + offset))

      assert_equal rows, AccountTransaction.count, "API #{offset} s from the file row"
      next unless rows == 1

      row = AccountTransaction.sole
      assert_equal ['dep-1', credited, { 'operation' => 'Deposit' }], [row.tx_id, row.transacted_at, row.raw_data], 'the file row stays, with the id'
    end
  end

  test 'a file deposit pairs with the stored API row it was credited after, and marks it' do
    seen = Time.utc(2021, 7, 1, 11, 6, 41)
    { 23 => 0, 10 * 60 => 0, 601 => 1, -1 => 1 }.each do |offset, imported|
      AccountTransaction.delete_all
      store(api_deposit(at: seen))
      counts = store(file_transfer(:deposit, '150.2', seen + offset))

      assert_equal imported, counts[:imported], "file #{offset} s after the API row"
      twin = AccountTransaction.find_by!(tx_id: 'dep-1').raw_data['file_twin_at']
      imported.zero? ? assert_equal((seen + offset).iso8601(6), twin) : assert_nil(twin)
    end
  end

  test 'an API withdrawal pairs with the gross file row at its completeTime' do
    apply = Time.utc(2021, 8, 23, 13, 8, 10)
    complete = Time.utc(2021, 8, 23, 13, 20, 44)
    cases = {
      'gross at completeTime' => [api_withdrawal(apply:, complete:), '10.44080476', complete, 1],
      'no fee, equal at completeTime' => [api_withdrawal(apply:, complete:, amount: '0.249', fee: '0'), '0.249', complete, 1],
      'net while a fee was charged' => [api_withdrawal(apply:, complete:), '10.43980476', complete, 2],
      'gross two seconds off' => [api_withdrawal(apply:, complete:), '10.44080476', complete + 2, 2],
      'unreadable completeTime' => [api_withdrawal(apply:, complete: 'soon'), '10.44080476', complete, 2],
      # No completeTime at all is not this venue's shape: the second-wide match as before.
      'no completeTime, no fee, same instant' => [api_withdrawal(apply:, complete: :none, amount: '0.249', fee: nil), '0.249', apply, 1],
      'no completeTime, gross at completion' => [api_withdrawal(apply:, complete: :none), '10.44080476', complete, 2]
    }
    cases.each do |name, (api, amount, at, rows)|
      AccountTransaction.delete_all
      store(file_transfer(:withdrawal, amount, at))
      store(api)
      assert_equal rows, AccountTransaction.count, name
    end
  end

  test 'whichever arrived first stands, with its own amount and fee' do
    apply = Time.utc(2021, 8, 23, 13, 8, 10)
    complete = Time.utc(2021, 8, 23, 13, 20, 44)
    store(file_transfer(:withdrawal, '10.44080476', complete))
    store(api_withdrawal(apply:, complete:))
    file_first = AccountTransaction.sole
    assert_equal ['wd-1', '10.44080476'.to_d, nil, complete],
                 [file_first.tx_id, file_first.base_amount, file_first.fee_amount, file_first.transacted_at]

    AccountTransaction.delete_all
    store(api_withdrawal(apply:, complete:))
    counts = store(file_transfer(:withdrawal, '10.44080476', complete))
    api_first = AccountTransaction.sole
    assert_equal 0, counts[:imported]
    assert_equal ['10.43980476'.to_d, '0.001'.to_d, apply], [api_first.base_amount, api_first.fee_amount, api_first.transacted_at]
  end

  # A claim is written onto the row, so the pairing survives between calls: the sync stores a venue's
  # history in chunks, and a file is imported on its own.
  test 'one file row is claimed by one API row, whatever the calls' do
    file_at = Time.utc(2021, 7, 3, 10, 5)
    a = api_deposit(at: Time.utc(2021, 7, 3, 10, 0), id: 'dep-a')
    b = api_deposit(at: Time.utc(2021, 7, 3, 10, 3), id: 'dep-b')
    [[a, b], [b, a]].each do |first, second|
      AccountTransaction.delete_all
      store(file_transfer(:deposit, '150.2', file_at))
      store(first)
      store(second)

      assert_equal [first[:tx_id], second[:tx_id]].sort, AccountTransaction.pluck(:tx_id).sort
      assert_equal file_at, AccountTransaction.find_by!(tx_id: first[:tx_id]).transacted_at
    end

    counts = store(file_transfer(:deposit, '150.2', file_at))
    assert_equal 0, counts[:imported], 'the same file row again'
    assert_nil AccountTransaction.find_by!(tx_id: 'dep-a').raw_data['file_twin_at'], 'the unclaimed API row stays unclaimed'
  end

  test 'two file rows for one API row are a visible double, not a guess' do
    store(file_transfer(:deposit, '150.2', Time.utc(2021, 7, 3, 10, 1)), file_transfer(:deposit, '150.2', Time.utc(2021, 7, 3, 10, 2)))
    store(api_deposit(at: Time.utc(2021, 7, 3, 10, 0)))
    assert_equal 3, AccountTransaction.count

    AccountTransaction.delete_all
    store(api_deposit(at: Time.utc(2021, 7, 3, 10, 0)))
    counts = store(file_transfer(:deposit, '150.2', Time.utc(2021, 7, 3, 10, 1)), file_transfer(:deposit, '150.2', Time.utc(2021, 7, 3, 10, 2)))
    assert_equal 2, counts[:imported], 'one file, two equal deposits beside one API row'
  end

  test 'an unrelated row in the same file is not a rival for the API row' do
    at = Time.utc(2021, 7, 3, 10, 5)
    store(api_deposit(at: Time.utc(2021, 7, 3, 10, 0)))
    counts = store(file_transfer(:deposit, '150.2', at), file_transfer(:deposit, '150.2', at, coin: 'BUSD'),
                   file_transfer(:withdrawal, '150.2', at, coin: 'USDT'))
    assert_equal 2, counts[:imported], 'the USDT deposit pairs; the BUSD deposit and the withdrawal are their own'
    assert_equal at.iso8601(6), AccountTransaction.find_by!(tx_id: 'dep-1').raw_data['file_twin_at']
  end

  test 'a file row paired on an earlier import is not a rival on the next' do
    store(api_deposit(at: Time.utc(2021, 7, 3, 10, 0), id: 'dep-a'))
    assert_equal 0, store(file_transfer(:deposit, '150.2', Time.utc(2021, 7, 3, 10, 2)))[:imported]
    store(api_deposit(at: Time.utc(2021, 7, 3, 10, 1), id: 'dep-b'))

    counts = store(file_transfer(:deposit, '150.2', Time.utc(2021, 7, 3, 10, 2)), file_transfer(:deposit, '150.2', Time.utc(2021, 7, 3, 10, 3)))
    assert_equal 0, counts[:imported], 'the expanded file adds nothing'
    assert_equal Time.utc(2021, 7, 3, 10, 3).iso8601(6), AccountTransaction.find_by!(tx_id: 'dep-b').raw_data['file_twin_at']
  end

  test 'a file row claims an API row once; a different file row later is its own' do
    store(api_deposit(at: Time.utc(2021, 7, 3, 10, 0)))
    assert_equal 0, store(file_transfer(:deposit, '150.2', Time.utc(2021, 7, 3, 10, 5)))[:imported]
    assert_equal 0, store(file_transfer(:deposit, '150.2', Time.utc(2021, 7, 3, 10, 5)))[:imported], 'the same row again'
    assert_equal 1, store(file_transfer(:deposit, '150.2', Time.utc(2021, 7, 3, 10, 6)))[:imported]
  end

  test 'a claim pairs only its own coin, amount and type' do
    at = Time.utc(2021, 7, 3, 10, 5)
    store(api_deposit(at: Time.utc(2021, 7, 3, 10, 0)))
    store(file_transfer(:deposit, '150.2', at))

    counts = store(file_transfer(:deposit, '150.2', at, coin: 'BUSD'), file_transfer(:deposit, '150.3', at),
                   file_transfer(:withdrawal, '150.2', at, coin: 'USDT'))
    assert_equal 3, counts[:imported]
  end

  test 'a row without the file or the API shape is matched as before' do
    at = Time.utc(2021, 7, 3, 10, 0)
    store(file_transfer(:deposit, '150.2', at + 46).merge(raw_data: {}))
    store(api_deposit(at:))
    assert_equal 2, AccountTransaction.count, 'an id-less row that is not a file transfer'

    AccountTransaction.delete_all
    store(file_transfer(:deposit, '150.2', at + 46))
    store(api_deposit(at:).merge(tx_id: ''), api_deposit(at: at + 46.4, id: nil))
    assert_equal 2, AccountTransaction.count, 'a blank id claims nothing: 46 s apart is two rows, 0.4 s is one'
  end

  test 'a claim that loses a race is stored, and an id stored meanwhile is a duplicate' do
    store(file_transfer(:deposit, '150.2', Time.utc(2021, 7, 3, 10, 5)))
    sync = AccountTransactionSync.new(@api_key)
    sync.expects(:take!).twice.returns(false)
    assert_equal 1, sync.store!([api_deposit(at: Time.utc(2021, 7, 3, 10, 0))])[:imported]

    AccountTransaction.delete_all
    store(file_transfer(:deposit, '150.2', Time.utc(2021, 7, 3, 10, 5)))
    sync = AccountTransactionSync.new(@api_key)
    sync.expects(:take!).raises(ActiveRecord::RecordNotUnique)
    assert_equal({ imported: 0, duplicates: 1 }, sync.store!([api_deposit(at: Time.utc(2021, 7, 3, 10, 0))]).slice(:imported, :duplicates))
  end

  test 'a claim leaves the ledger a fresh walk computes' do
    freeze_time
    store(file_transfer(:deposit, '150.2', Time.utc(2021, 7, 3, 10, 5)))
    before = Tracker::Ledger.compute!(@user)
    updated = AccountTransaction.sole.updated_at
    store(api_deposit(at: Time.utc(2021, 7, 3, 10, 0)))

    assert_equal updated, AccountTransaction.sole.updated_at
    Rails.cache.clear
    assert_equal Marshal.dump(before), Marshal.dump(Tracker::Ledger.compute!(@user))
  end

  # ---- Binance's export and its API on one dust conversion ------------------------------------------
  # The API books the BNB in gross, with the 2% charge beside it; the export writes it net, up to a
  # second after operateTime. A conversion is paired whole, coin leg and BNB leg, or not at all.

  def api_dust(at:, coin: 'USDT', amount: '25.47403194', gross: '0.07147454', charge: '0.00142949', id: 78_240_438_808)
    raw = { 'fromAsset' => coin, 'amount' => amount, 'transferedAmount' => gross, 'serviceChargeAmount' => charge,
            'operateTime' => (at.to_f * 1000).to_i, 'transId' => id, 'targetAsset' => 'BNB' }
    leg = { quote_currency: nil, quote_amount: nil, group_id: "dust_#{id}", description: 'Dust conversion', transacted_at: at, raw_data: raw }
    [leg.merge(entry_type: :swap_out, base_currency: coin, base_amount: amount.to_d, fee_currency: nil, fee_amount: nil, tx_id: "dust-#{id}-out"),
     leg.merge(entry_type: :swap_in, base_currency: 'BNB', base_amount: gross.to_d, fee_currency: 'BNB', fee_amount: charge.to_d,
               tx_id: "dust-#{id}-in")]
  end

  def file_dust(at:, coin: 'USDT', amount: '25.47403194', net: '0.07004505', group: "dustcsv_#{coin} to BNB_#{at.to_i}")
    leg = { quote_currency: nil, quote_amount: nil, fee_currency: nil, fee_amount: nil, tx_id: nil, group_id: group,
            description: nil, transacted_at: at, raw_data: {} }
    [leg.merge(entry_type: :swap_out, base_currency: coin, base_amount: amount.to_d),
     leg.merge(entry_type: :swap_in, base_currency: 'BNB', base_amount: net.to_d)]
  end

  test 'an API dust conversion pairs with the net file group up to a second either side, and claims it' do
    file_at = Time.utc(2021, 9, 21, 19, 22, 1)
    { -1 => 2, 0 => 2, 1 => 2, -2 => 4, 2 => 4 }.each do |offset, rows|
      AccountTransaction.delete_all
      store(*file_dust(at: file_at))
      store(*api_dust(at: file_at + offset))

      assert_equal rows, AccountTransaction.count, "API #{offset} s from the file group"
      next unless rows == 2

      assert_equal [['USDT', '25.47403194'.to_d, 'dust-78240438808-out'], ['BNB', '0.07004505'.to_d, 'dust-78240438808-in']],
                   AccountTransaction.order(:id).map { |row| [row.base_currency, row.base_amount, row.tx_id] }, 'the file rows stay, with the ids'
    end
  end

  test 'a file dust group pairs with the stored API conversion, and marks it' do
    api_at = Time.utc(2021, 9, 21, 19, 22, 0)
    { 1 => 0, 0 => 0, -1 => 0, 2 => 2 }.each do |offset, imported|
      AccountTransaction.delete_all
      store(*api_dust(at: api_at))
      counts = store(*file_dust(at: api_at + offset))

      assert_equal imported, counts[:imported], "file #{offset} s after the API conversion"
      twins = AccountTransaction.where(tx_id: %w[dust-78240438808-out dust-78240438808-in]).map { |row| row.raw_data['file_twin_at'] }
      assert_equal [imported.zero? ? (api_at + offset).iso8601(6) : nil] * 2, twins
      assert_equal 0, store(*file_dust(at: api_at + offset))[:imported], 'the same file again'
    end
  end

  test 'a dust conversion whose BNB legs disagree pairs neither leg' do
    at = Time.utc(2021, 9, 21, 19, 22, 0)
    store(*file_dust(at:, net: '0.07147454')) # gross where the export is net
    store(*api_dust(at:))
    assert_equal 4, AccountTransaction.count

    AccountTransaction.delete_all
    store(*api_dust(at:))
    assert_equal 2, store(*file_dust(at:, net: '0.07'))[:imported]
  end

  test 'a dust leg on its own pairs with nothing' do
    at = Time.utc(2021, 9, 21, 19, 22, 0)
    store(file_dust(at:).first)
    store(*api_dust(at:))
    assert_equal 3, AccountTransaction.count

    AccountTransaction.delete_all
    store(*api_dust(at:))
    assert_equal 1, store(file_dust(at:).last)[:imported]
  end

  test 'an export batch pairs each coin with its own API conversion' do
    at = Time.utc(2021, 7, 1, 11, 20, 49)
    batch = file_dust(at:, group: "dustcsv_#{at.to_i}") +
            file_dust(at:, coin: 'ADA', amount: '0.792868', net: '0.00355388', group: "dustcsv_#{at.to_i}")
    store(*batch)
    store(*api_dust(at:, coin: 'ADA', amount: '0.792868', gross: '0.0036264', charge: '0.00007252', id: 67_771_421_609))

    assert_equal([['USDT', nil], ['BNB', nil], %w[ADA dust-67771421609-out], %w[BNB dust-67771421609-in]],
                 AccountTransaction.order(:id).map { |row| [row.base_currency, row.tx_id] })

    AccountTransaction.delete_all
    store(*api_dust(at:, coin: 'ADA', amount: '0.792868', gross: '0.0036264', charge: '0.00007252', id: 67_771_421_609))
    assert_equal 2, store(*batch)[:imported], 'the USDT pair, which the API never saw'
  end

  # The importer collapses the second equal credit; written as two groups, each still pairs with its own.
  test 'two coins whose BNB credits are equal each pair with their own group' do
    at = Time.utc(2022, 6, 13, 12, 29, 41)
    (file_dust(at:, amount: '0.9', net: '0.000432') + file_dust(at:, coin: 'XRP', amount: '1.5', net: '0.000432')).each do |leg|
      AccountTransaction.create!(leg.merge(user: @user, exchange: @exchange, api_key: @api_key))
    end
    store(*api_dust(at:, coin: 'XRP', amount: '1.5', gross: '0.000441', charge: '0.000009', id: 2),
          *api_dust(at:, amount: '0.9', gross: '0.000441', charge: '0.000009', id: 1))

    assert_equal([['USDT', 'dust-1-out'], ['BNB', 'dust-1-in'], ['XRP', 'dust-2-out'], ['BNB', 'dust-2-in']],
                 AccountTransaction.order(:id).map { |row| [row.base_currency, row.tx_id] })
  end

  test 'a file dust group pairs only with a whole API conversion' do
    at = Time.utc(2021, 9, 21, 19, 22, 0)
    store(api_dust(at:).first) # a sync cut off after the coin leg
    assert_equal 2, store(*file_dust(at:))[:imported], 'the BNB credit is never lost'
    assert_nil AccountTransaction.find_by!(tx_id: 'dust-78240438808-out').raw_data['file_twin_at']
  end

  test 'a dust amount the column rounds still pairs, as the database compares it' do
    at = Time.utc(2021, 9, 21, 19, 22, 0)
    store(*file_dust(at:, amount: '123456789.12345678'))
    store(*api_dust(at:, amount: '123456789.12345678'))
    assert_equal %w[dust-78240438808-in dust-78240438808-out], AccountTransaction.pluck(:tx_id).sort
  end

  test 'two dust candidates are a visible double, not a guess' do
    at = Time.utc(2021, 9, 21, 19, 22, 0)
    store(*file_dust(at:), *file_dust(at: at + 1, group: 'dustcsv_other'))
    store(*api_dust(at:))
    assert_equal 6, AccountTransaction.count, 'two file groups for one API conversion'

    AccountTransaction.delete_all
    store(*api_dust(at:), *api_dust(at: at + 1, id: 2))
    assert_equal 2, store(*file_dust(at:))[:imported], 'two API conversions for one file group'
  end

  test 'one file dust group pairs with one API conversion, whatever the calls' do
    at = Time.utc(2021, 9, 21, 19, 22, 0)
    store(*file_dust(at:))
    store(*api_dust(at:, id: 1))
    store(*api_dust(at:, id: 2))
    assert_equal %w[dust-1-in dust-1-out dust-2-in dust-2-out], AccountTransaction.pluck(:tx_id).sort

    AccountTransaction.delete_all
    store(*file_dust(at:))
    store(api_dust(at:, id: 1).last)
    store(api_dust(at:, id: 1).first, *api_dust(at:, id: 2))
    assert_equal %w[dust-1-in dust-1-out dust-2-in dust-2-out], AccountTransaction.pluck(:tx_id).sort, 'one leg per call'
  end

  # The pairings follow the run rules too: a row this run stored is no evidence, the rest count one for one.
  test 'two identical dust sweeps in one second of a file are both stored, and only once' do
    at = Time.utc(2021, 9, 21, 19, 22, 0)
    assert_equal({ imported: 4, duplicates: 0 }, store(*file_dust(at:), *file_dust(at:)).slice(:imported, :duplicates))
    assert_equal({ imported: 0, duplicates: 4 }, store(*file_dust(at:), *file_dust(at:)).slice(:imported, :duplicates), 'the same file again')
  end

  test 'two identical file transfers in one batch are both stored, and only once' do
    row = file_transfer(:deposit, '150.2', Time.utc(2021, 7, 1, 11, 7, 4))
    assert_equal({ imported: 2, duplicates: 0 }, store(row, row.dup).slice(:imported, :duplicates))
    assert_equal({ imported: 0, duplicates: 2 }, store(row, row.dup).slice(:imported, :duplicates), 'the same file again')
  end

  # One stored id-less row the pairing does not govern, exactly a second on (across sources), is one event: of two
  # identical API conversions or deposits, it accounts for the first only.
  test 'a stored row outside the pairing counts one for one against identical API dust and deposits' do
    at = Time.utc(2021, 9, 21, 19, 22, 0)
    create(:account_transaction, api_key: @api_key, exchange: @exchange, entry_type: :swap_out, base_currency: 'USDT',
                                 base_amount: '25.47403194'.to_d, tx_id: nil, transacted_at: at + 1, raw_data: {})
    create(:account_transaction, api_key: @api_key, exchange: @exchange, entry_type: :deposit, base_currency: 'USDT',
                                 base_amount: '150.2'.to_d, tx_id: nil, transacted_at: at + 1, raw_data: {})
    store(*api_dust(at:, id: 1), *api_dust(at:, id: 2), api_deposit(at:, id: 'dep-1'), api_deposit(at:, id: 'dep-2'))

    assert_equal [nil, nil, 'dep-2', 'dust-1-in', 'dust-2-in', 'dust-2-out'], AccountTransaction.pluck(:tx_id).sort_by(&:to_s)
  end

  # A Convert out of cash used to be read from the file as a purchase; it is now the swap pair the
  # API books. A file imported under the old reading holds the purchase, and importing it again must
  # not add the pair on top: a swap leg is the purchase (or sale) it replaced when the coin leg is
  # that row's base and the cash leg is its quote.
  test 'a file Convert stored as a purchase or a sale is not stored again as a swap pair' do
    at = Time.utc(2021, 9, 20, 6, 22, 54)
    leg = { quote_currency: nil, quote_amount: nil, fee_currency: nil, fee_amount: nil, tx_id: nil,
            description: nil, raw_data: {} }
    bought = leg.merge(entry_type: :buy, base_currency: 'LTC', base_amount: '0.89568816'.to_d,
                       quote_currency: 'USDT', quote_amount: 150.to_d, group_id: nil, transacted_at: at)
    sold = leg.merge(entry_type: :sell, base_currency: 'BNB', base_amount: '0.75'.to_d,
                     quote_currency: 'USDC', quote_amount: 450.to_d, group_id: nil, transacted_at: at + 1.day)
    AccountTransactionSync.new(@api_key).store!([bought, sold])

    pair = [leg.merge(entry_type: :swap_out, base_currency: 'USDT', base_amount: 150.to_d, group_id: 'swapcsv_1', transacted_at: at),
            leg.merge(entry_type: :swap_in, base_currency: 'LTC', base_amount: '0.89568816'.to_d, group_id: 'swapcsv_1', transacted_at: at),
            leg.merge(entry_type: :swap_out, base_currency: 'BNB', base_amount: '0.75'.to_d, group_id: 'swapcsv_2', transacted_at: at + 1.day),
            leg.merge(entry_type: :swap_in, base_currency: 'USDC', base_amount: 450.to_d, group_id: 'swapcsv_2', transacted_at: at + 1.day)]
    counts = AccountTransactionSync.new(@api_key).store!(pair)

    assert_equal({ imported: 0, duplicates: 4 }, counts.slice(:imported, :duplicates))
    assert_equal 2, AccountTransaction.where(user: @user).count
  end

  # The pair is matched as a WHOLE against one stored trade — its coin as that trade's base and its
  # cash as that trade's quote — never one leg at a time: a purchase of 1 LTC for 100 USDT is not
  # the Convert that received 1 LTC for 200 USDC in the same second, and skipping the LTC leg alone
  # would leave a one-legged swap in the ledger.
  test 'a legacy purchase matches a file pair as a whole, never one leg of it' do
    at = Time.utc(2021, 9, 20, 6, 22, 54)
    leg = { quote_currency: nil, quote_amount: nil, fee_currency: nil, fee_amount: nil, tx_id: nil,
            description: nil, raw_data: {}, transacted_at: at }
    AccountTransactionSync.new(@api_key).store!([leg.merge(entry_type: :buy, base_currency: 'LTC', base_amount: 1.to_d,
                                                           quote_currency: 'USDT', quote_amount: 100.to_d, group_id: nil)])

    other = [leg.merge(entry_type: :swap_out, base_currency: 'USDC', base_amount: 200.to_d, group_id: 'swapcsv_a'),
             leg.merge(entry_type: :swap_in, base_currency: 'LTC', base_amount: 1.to_d, group_id: 'swapcsv_a')]
    same = [leg.merge(entry_type: :swap_out, base_currency: 'USDT', base_amount: 100.to_d, group_id: 'swapcsv_b'),
            leg.merge(entry_type: :swap_in, base_currency: 'LTC', base_amount: 1.to_d, group_id: 'swapcsv_b')]

    assert_equal({ imported: 2, duplicates: 0 }, AccountTransactionSync.new(@api_key).store!(other).slice(:imported, :duplicates),
                 'a different Convert, both legs')
    assert_equal({ imported: 0, duplicates: 2 }, AccountTransactionSync.new(@api_key).store!(same).slice(:imported, :duplicates),
                 'the purchase, both legs')
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
    alpaca_key.update_column(:last_sync_error, 'StandardError: API error')
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
    # A skipped row is our importer's bug, not a failed sync: the run completed, the watermark is
    # held back so the row is retried, and only the error log calls it out. Keeping the sync error
    # set here would stamp a permanent "data missing" banner on every future tax report.
    assert_nil alpaca_key.last_sync_error
  end

  # This used to assert the opposite: that a second key on one exchange meant a second SUB-ACCOUNT,
  # so an id-less row had to be scoped to the key that wrote it. Two things undid that premise.
  #
  # The schema cannot express it — `ApiKey` is unique per (user, exchange, key_type), so a user
  # cannot hold two trading keys for two sub-accounts in the first place. And a venue's history now
  # genuinely accumulates under DIFFERENT keys over time: a key is replaced (its rows are nullified),
  # rotated, or superseded by a reading key. Key-scoped, each of those made a re-read of the same
  # history land a second time — a doubled ledger, and every P/L on the page silenced, because a
  # balance and a ledger that disagree can state nothing.
  test 'the nil tx_id fallback recognises a row whichever key on the venue wrote it' do
    second_key = create(:api_key, user: @user, exchange: @exchange, key_type: :read_only)
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
    assert_equal 0, second_result.data, 'the same reward, already stored'
    assert_equal [@api_key.id], AccountTransaction.where(tx_id: nil).pluck(:api_key_id)
  end

  # ---- a split, said out loud in the bot's own log ----
  #
  # A split changes a share count without anything being bought or sold. Nothing in a bot's feed
  # explains that, so the count simply appears to jump. One info line, on the bots that were
  # actually holding the symbol at the time, dated when the split happened.

  # Relative, not a literal: `announce_split` writes nothing for a split older than the feed's
  # retention, so a fixed date would quietly stop exercising any of this once the clock passed it.
  def split_at
    @split_at ||= 10.days.ago.change(usec: 0)
  end

  def split_entry(symbol: 'KLAC', ratio: '10:1', tx_id: 'split-1')
    { entry_type: :adjustment, base_currency: symbol, base_amount: 90,
      quote_currency: nil, quote_amount: nil, fee_currency: nil, fee_amount: nil,
      tx_id: tx_id, group_id: nil, description: "Split (#{symbol}) #{ratio}",
      transacted_at: split_at,
      raw_data: { 'corporate_action' => 'split', 'split_ratio' => ratio }.compact }
  end

  def holder(symbol: 'KLAC', at: split_at - 1.day, exchange: @exchange, user: @user, **attributes)
    asset = Asset.find_by(symbol: symbol) || create(:asset, external_id: symbol.downcase, symbol: symbol)
    usd = Asset.find_by(symbol: 'USD') || create(:asset, :usd)
    bot = create(:dca_single_asset, user: user, exchange: exchange,
                                    base_asset: asset, quote_asset: usd, with_api_key: false)
    create(:transaction, bot: bot, exchange: exchange, base: symbol, quote: 'USD',
                         created_at: at, **attributes)
    bot
  end

  def sync_split!(entry = split_entry)
    @exchange.stubs(:get_ledger).returns(Result::Success.new([entry]))
    AccountTransactionSync.new(@api_key).sync!
  end

  test 'a split is logged on a bot that was holding the symbol' do
    bot = holder

    sync_split!

    log = bot.bot_activity_logs.sole
    assert_equal 'asset_split', log.event
    assert_equal 'info', log.level
    assert_equal 'KLAC', log.details['base']
    assert_equal '10:1', log.details['ratio']
  end

  test 'the line is dated at the split, not at the sync' do
    bot = holder

    sync_split!

    assert_equal split_at, bot.bot_activity_logs.sole.created_at
  end

  test 'a split with no derivable ratio still says it happened' do
    bot = holder

    sync_split!(split_entry(ratio: nil))

    assert_nil bot.bot_activity_logs.sole.details['ratio']
  end

  test 'a bot that only bought after the split is left alone' do
    bot = holder(at: split_at + 1.day)

    sync_split!

    assert_empty bot.bot_activity_logs
  end

  test 'an order that never executed is not a holding' do
    failed = holder(status: :failed, amount_exec: nil, quote_amount_exec: nil)
    skipped = holder(status: :skipped)
    unfilled = holder(external_status: :cancelled, amount_exec: nil, quote_amount_exec: nil)

    sync_split!

    assert_empty failed.bot_activity_logs
    assert_empty skipped.bot_activity_logs
    assert_empty unfilled.bot_activity_logs
  end

  test 'an order cancelled after filling part way still leaves a holding' do
    bot = holder(external_status: :cancelled, amount_exec: 0.4)

    sync_split!

    assert_equal 1, bot.bot_activity_logs.count, 'the shares it did fill are still owned'
  end

  test 'another symbol, another venue and another user are all left alone' do
    other_symbol = holder(symbol: 'AAPL')
    other_venue = holder(exchange: create(:kraken_exchange))
    other_user = holder(user: create(:user))

    sync_split!

    assert_empty other_symbol.bot_activity_logs
    assert_empty other_venue.bot_activity_logs
    assert_empty other_user.bot_activity_logs
  end

  test 'an ordinary ledger row logs nothing' do
    bot = holder(symbol: 'BTC')

    @exchange.stubs(:get_ledger).returns(Result::Success.new(@ledger_entries))
    AccountTransactionSync.new(@api_key).sync!

    assert_empty bot.bot_activity_logs
  end

  test 'an adjustment with no provenance logs nothing' do
    # An imported row: `Import::DeltabadgerCsv` carries no raw_data, so nothing says it was a split.
    bot = holder

    sync_split!(split_entry.merge(raw_data: {}))

    assert_empty bot.bot_activity_logs
  end

  test 'a second sync over the same window does not repeat the line' do
    bot = holder

    sync_split!
    sync_split!

    assert_equal 1, bot.bot_activity_logs.count
  end

  test 'a bot that had already sold out is not told about the split' do
    bot = holder
    create(:transaction, bot: bot, exchange: @exchange, base: 'KLAC', quote: 'USD',
                         side: :sell, created_at: split_at - 2.hours)

    sync_split!

    assert_empty bot.bot_activity_logs, 'a position that was closed before the split is not affected'
  end

  test 'a bot that sold only part of its position is still told' do
    bot = holder(amount_exec: 1.0)
    create(:transaction, bot: bot, exchange: @exchange, base: 'KLAC', quote: 'USD',
                         side: :sell, amount_exec: 0.4, created_at: split_at - 2.hours)

    sync_split!

    assert_equal 1, bot.bot_activity_logs.count
  end

  # The feed keeps 90 days. A split older than that has no trades left beside it to explain, and
  # the line would be deleted by the next prune anyway.
  test 'a split older than the feed keeps is not written at all' do
    bot = holder(at: 2.years.ago)

    sync_split!(split_entry.merge(transacted_at: 1.year.ago))

    assert_empty bot.bot_activity_logs
  end

  # Alpaca can ship the add leg before the remove leg exists. The standalone leg imports without a
  # ratio; the merged pair imports later and knows one. That is one split, so it is one line.
  test 'a split that arrives in two passes ends up as one line, with the ratio' do
    bot = holder

    sync_split!(split_entry(ratio: nil, tx_id: 'split-add').merge(base_amount: 90))
    sync_split!(split_entry(tx_id: 'split-remove'))

    log = bot.bot_activity_logs.sole
    assert_equal '10:1', log.details['ratio']
  end

  test 'two symbols splitting on the same day are two separate lines' do
    bot = holder
    aapl = create(:asset, external_id: 'aapl', symbol: 'AAPL')
    create(:ticker, exchange: @exchange, base_asset: aapl, quote_asset: Asset.find_by(symbol: 'USD'))
    create(:transaction, bot: bot, exchange: @exchange, base: 'AAPL', quote: 'USD',
                         created_at: split_at - 1.day)

    @exchange.stubs(:get_ledger).returns(
      Result::Success.new([split_entry, split_entry(symbol: 'AAPL', ratio: '4:1', tx_id: 'split-2')])
    )
    AccountTransactionSync.new(@api_key).sync!

    assert_equal %w[AAPL KLAC], bot.bot_activity_logs.map { |log| log.details['base'] }.sort
  end

  # A bot whose rows already crossed an earlier split holds as-traded buys and restated sells, so
  # the two are in different units and the sum can read negative. That is not a flat position — it
  # is proof the bot has been through exactly the thing this line exists to explain.
  test 'a position whose units no longer agree is still told' do
    bot = holder(amount_exec: 1.0)
    create(:transaction, bot: bot, exchange: @exchange, base: 'KLAC', quote: 'USD',
                         side: :sell, amount_exec: 5.0, created_at: split_at - 2.hours)

    sync_split!

    assert_equal 1, bot.bot_activity_logs.count
  end

  test 'a bot on another venue, and one that moved here later, are judged on where they traded' do
    kraken = create(:kraken_exchange)
    elsewhere = holder(exchange: kraken)
    # Moved to this venue after the fact — set directly, since the point under test is the stored
    # state, not the move. The orders it placed on Kraken are still Kraken's.
    elsewhere.update_columns(exchange_id: @exchange.id)

    sync_split!

    assert_empty elsewhere.bot_activity_logs
  end

  # One share bought before a 2-for-1, one of the resulting two sold after it: the as-traded rows
  # sum to zero and a share is still held.
  test 'a zero that spans an earlier split is not a flat position' do
    bot = holder(at: split_at - 30.days, amount_exec: 1.0)
    create(:account_transaction, user: @user, api_key: @api_key, exchange: @exchange,
                                 entry_type: :adjustment, base_currency: 'KLAC', base_amount: 1,
                                 quote_currency: nil, quote_amount: nil, tx_id: 'earlier-split',
                                 description: 'Split (KLAC) 2:1', transacted_at: split_at - 20.days,
                                 raw_data: { 'corporate_action' => 'split', 'split_ratio' => '2:1' })
    create(:transaction, bot: bot, exchange: @exchange, base: 'KLAC', quote: 'USD',
                         side: :sell, amount_exec: 1.0, created_at: split_at - 10.days)

    sync_split!

    assert_equal 1, bot.bot_activity_logs.count
  end

  test 'a bot that opened and closed entirely after the earlier split is flat and stays quiet' do
    # The account has been through an earlier split, but this bot's own rows are all on one side
    # of it, so its zero is a real zero.
    create(:account_transaction, user: @user, api_key: @api_key, exchange: @exchange,
                                 entry_type: :adjustment, base_currency: 'KLAC', base_amount: 1,
                                 quote_currency: nil, quote_amount: nil, tx_id: 'earlier-split',
                                 description: 'Split (KLAC) 2:1', transacted_at: split_at - 20.days,
                                 raw_data: { 'corporate_action' => 'split', 'split_ratio' => '2:1' })
    bot = holder(at: split_at - 10.days, amount_exec: 1.0)
    create(:transaction, bot: bot, exchange: @exchange, base: 'KLAC', quote: 'USD',
                         side: :sell, amount_exec: 1.0, created_at: split_at - 5.days)

    sync_split!

    assert_empty bot.bot_activity_logs
  end

  # ---- the ledger the split changes ----
  #
  # The activity line is a courtesy; the restatement is not. Every cached metrics hash a bot has is
  # computed from a position this event moves, and `Bot::Rebalancer` reads one of them WITHOUT
  # forcing — so a stale hash is a sell sized off a tenth of a position.

  def cached_metrics_keys(bot)
    [bot.send(:metrics_cache_key),
     bot.send(:metrics_with_current_prices_cache_key),
     bot.send(:metrics_with_current_prices_and_candles_cache_key)]
  end

  # The test environment runs a null store, which would make every assertion here vacuous.
  # Returns the keys it seeded: a restatement moves them, so they have to be captured first.
  def seed_caches(bot)
    Rails.stubs(:cache).returns(@memory_cache ||= ActiveSupport::Cache::MemoryStore.new)
    cached_metrics_keys(bot).each do |key|
      Rails.cache.write(key, { stale: true })
      assert_equal({ stale: true }, Rails.cache.read(key))
    end
    cached_metrics_keys(bot)
  end

  test 'a split drops every cached metrics hash of a bot that traded the symbol' do
    bot = holder
    stale_keys = seed_caches(bot)

    sync_split!

    stale_keys.each { |key| assert_nil Rails.cache.read(key), "#{key} survived" }
  end

  test 'and moves the keys past anything a pre-split walk could still write' do
    bot = holder
    stale_keys = seed_caches(bot)

    sync_split!

    assert_equal 1, bot.reload.restatement_generation
    assert_empty (stale_keys & cached_metrics_keys(bot)), 'a pre-split writer can only reach the old keys'
  end

  test 'a page already open is told to redraw' do
    bot = holder
    Bot::BroadcastMetricsUpdateJob.expects(:perform_later).with(bot)

    sync_split!
  end

  test 'it drops them for a split too old for the feed to keep, which says nothing' do
    bot = holder(at: 2.years.ago)
    seed_caches(bot)

    stale = cached_metrics_keys(bot).first
    sync_split!(split_entry.merge(transacted_at: 1.year.ago))

    assert_empty bot.bot_activity_logs
    assert_nil Rails.cache.read(stale)
  end

  test 'it drops them for a bot whose position reads flat, which is told nothing' do
    bot = holder
    create(:transaction, bot: bot, exchange: @exchange, base: 'KLAC', quote: 'USD',
                         side: :sell, created_at: split_at - 2.hours)
    seed_caches(bot)

    stale = cached_metrics_keys(bot).first
    sync_split!

    assert_empty bot.bot_activity_logs
    assert_nil Rails.cache.read(stale)
  end

  test 'it leaves a bot that traded the symbol on another venue alone' do
    elsewhere = holder(exchange: create(:kraken_exchange))
    seed_caches(elsewhere)

    sync_split!

    assert_equal({ stale: true }, Rails.cache.read(cached_metrics_keys(elsewhere).first))
  end

  test 'an ordinary ledger row drops nothing' do
    bot = holder
    seed_caches(bot)

    @exchange.stubs(:get_ledger).returns(Result::Success.new(@ledger_entries))
    AccountTransactionSync.new(@api_key).sync!

    assert_equal({ stale: true }, Rails.cache.read(cached_metrics_keys(bot).first))
  end

  # A corporate action dated ahead of today is imported once and takes effect later. No sync comes
  # back to it — by then the row is a duplicate — so the bump has to be booked for that moment.
  test 'an action that has not happened yet books its own second bump' do
    holder
    effective = 3.days.from_now.change(usec: 0)
    Bot::ExpireRestatedMetricsJob.expects(:set).with(wait_until: effective)
                                 .returns(stub(perform_later: true))

    sync_split!(split_entry.merge(transacted_at: effective))
  end

  test 'an action already in effect books nothing' do
    holder
    Bot::ExpireRestatedMetricsJob.expects(:set).never

    sync_split!
  end

  # The row is stored by the time these run, and a later sync reads it as a duplicate and never
  # returns — so a failure here must not take the rest of the batch down with it.
  test 'a failure in the split side effects is logged, not raised' do
    holder
    AccountTransactionSync.stubs(:expire_restated_bots).raises(StandardError, 'boom')
    Rails.logger.expects(:error).with(regexp_matches(/Split side effects failed for KLAC/))

    assert_predicate sync_split!, :success?
    assert_equal 1, AccountTransaction.where(entry_type: :adjustment).count, 'the row still landed'
  end

  # ── the asset each row moved ──────────────────────────────────────────────────────────────────

  def list_btc_usd
    @bitcoin = create(:asset, :bitcoin)
    @usd = create(:asset, :usd)
    create(:ticker, exchange: @exchange, base_asset: @bitcoin, quote_asset: @usd, base: 'BTC', quote: 'USD', ticker: 'BTCUSD')
  end

  test 'store! records the asset the venue lists for each row' do
    list_btc_usd
    @exchange.stubs(:get_ledger).returns(Result::Success.new(@ledger_entries))

    AccountTransactionSync.new(@api_key).sync!

    assert_equal @bitcoin, AccountTransaction.find_by(tx_id: 'trade-1').base_asset
    assert_equal @usd, AccountTransaction.find_by(tx_id: 'deposit-1').base_asset, 'a quote spelling is the venue\'s name too'
  end

  test 'a resolver error still saves every row, with no asset' do
    list_btc_usd
    @exchange.stubs(:get_ledger).returns(Result::Success.new(@ledger_entries))
    @exchange.stubs(:ledger_asset_ids).raises(StandardError, 'boom')
    Rails.logger.expects(:error).with(regexp_matches(/could not resolve.*boom/i)).at_least_once

    assert_predicate AccountTransactionSync.new(@api_key).sync!, :success?
    assert_equal [nil, nil], AccountTransaction.order(:id).pluck(:base_asset_id)
  end

  # A first sync can run before the venue's listings have arrived. The next sync — daily, whether or
  # not anyone visits — asks again for rows stored within the window.
  test 'a row stored before its listing existed is resolved by the next sync' do
    @exchange.stubs(:get_ledger).returns(Result::Success.new(@ledger_entries))
    AccountTransactionSync.new(@api_key).sync!
    assert_nil AccountTransaction.find_by(tx_id: 'trade-1').base_asset_id

    list_btc_usd
    AccountTransactionSync.new(@api_key).sync!

    assert_equal @bitcoin.id, AccountTransaction.find_by(tx_id: 'trade-1').base_asset_id
  end

  # Past the window a name the venue lists NOW is not evidence of what it meant when the row was
  # stored: a later relisting of a dead spelling must not be stamped onto it.
  test 'a row stored longer ago than the window is left as it is' do
    @exchange.stubs(:get_ledger).returns(Result::Success.new(@ledger_entries))
    travel_to(8.days.ago) { AccountTransactionSync.new(@api_key).sync! }

    list_btc_usd
    AccountTransactionSync.new(@api_key).sync!

    assert_nil AccountTransaction.find_by(tx_id: 'trade-1').base_asset_id
  end

  test 'a recorded asset is never overwritten' do
    list_btc_usd
    other = create(:asset)
    @exchange.stubs(:get_ledger).returns(Result::Success.new(@ledger_entries.first(1)))
    AccountTransactionSync.new(@api_key).sync!
    AccountTransaction.update_all(base_asset_id: other.id)

    AccountTransactionSync.new(@api_key).sync!

    assert_equal other.id, AccountTransaction.sole.base_asset_id
  end
end
