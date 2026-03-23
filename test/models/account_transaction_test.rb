require 'test_helper'

class AccountTransactionTest < ActiveSupport::TestCase
  setup do
    @user = create(:user)
    @exchange = create(:binance_exchange)
    @api_key = create(:api_key, user: @user, exchange: @exchange)
  end

  # --- Validations ---

  test 'valid with all required attributes' do
    at = build(:account_transaction, api_key: @api_key, exchange: @exchange)
    assert at.valid?
  end

  test 'invalid without base_currency' do
    at = build(:account_transaction, api_key: @api_key, exchange: @exchange, base_currency: nil)
    assert_not at.valid?
    assert_includes at.errors[:base_currency], "can't be blank"
  end

  test 'invalid without base_amount' do
    at = build(:account_transaction, api_key: @api_key, exchange: @exchange, base_amount: nil)
    assert_not at.valid?
    assert_includes at.errors[:base_amount], "can't be blank"
  end

  test 'invalid without transacted_at' do
    at = build(:account_transaction, api_key: @api_key, exchange: @exchange, transacted_at: nil)
    assert_not at.valid?
    assert_includes at.errors[:transacted_at], "can't be blank"
  end

  test 'tx_id must be unique per exchange' do
    create(:account_transaction, api_key: @api_key, exchange: @exchange, tx_id: 'order-123')
    duplicate = build(:account_transaction, api_key: @api_key, exchange: @exchange, tx_id: 'order-123')
    assert_not duplicate.valid?
    assert_includes duplicate.errors[:tx_id], 'has already been taken'
  end

  test 'tx_id uniqueness is scoped to exchange' do
    other_exchange = create(:kraken_exchange)
    other_api_key = create(:api_key, user: @user, exchange: other_exchange)
    create(:account_transaction, api_key: @api_key, exchange: @exchange, tx_id: 'order-123')
    other = build(:account_transaction, api_key: other_api_key, exchange: other_exchange, tx_id: 'order-123')
    assert other.valid?
  end

  test 'allows nil tx_id (non-unique)' do
    create(:account_transaction, api_key: @api_key, exchange: @exchange, tx_id: nil)
    second = build(:account_transaction, api_key: @api_key, exchange: @exchange, tx_id: nil)
    assert second.valid?
  end

  # --- Enums ---

  test 'entry_type enum has all expected values' do
    expected = %w[buy sell swap_in swap_out deposit withdrawal staking_reward lending_interest airdrop mining fee other_income lost]
    assert_equal expected.sort, AccountTransaction.entry_types.keys.sort
  end

  test 'entry_type can be set and queried' do
    at = create(:account_transaction, api_key: @api_key, exchange: @exchange, entry_type: :deposit)
    assert at.deposit?
    assert_not at.buy?
  end

  # --- Associations ---

  test 'belongs to api_key' do
    at = create(:account_transaction, api_key: @api_key, exchange: @exchange)
    assert_equal @api_key, at.api_key
  end

  test 'belongs to exchange' do
    at = create(:account_transaction, api_key: @api_key, exchange: @exchange)
    assert_equal @exchange, at.exchange
  end

  test 'optionally belongs to bot_transaction' do
    at = create(:account_transaction, api_key: @api_key, exchange: @exchange, bot_transaction: nil)
    assert_nil at.bot_transaction

    bot = create(:dca_single_asset, user: @user, exchange: @exchange, with_api_key: false)
    bot_tx = create(:transaction, bot: bot, exchange: @exchange)
    at.update!(bot_transaction: bot_tx)
    assert_equal bot_tx, at.reload.bot_transaction
  end

  # --- Scopes ---

  test 'for_user returns only transactions for the given user' do
    other_user = create(:user)
    other_exchange = create(:kraken_exchange)
    other_api_key = create(:api_key, user: other_user, exchange: other_exchange)

    mine = create(:account_transaction, api_key: @api_key, exchange: @exchange)
    _theirs = create(:account_transaction, api_key: other_api_key, exchange: other_exchange)

    results = AccountTransaction.for_user(@user)
    assert_includes results, mine
    assert_equal 1, results.count
  end

  test 'for_exchange filters by exchange' do
    other_exchange = create(:kraken_exchange)
    other_api_key = create(:api_key, user: @user, exchange: other_exchange)

    binance_tx = create(:account_transaction, api_key: @api_key, exchange: @exchange)
    _kraken_tx = create(:account_transaction, api_key: other_api_key, exchange: other_exchange)

    results = AccountTransaction.for_exchange(@exchange)
    assert_includes results, binance_tx
    assert_equal 1, results.count
  end

  test 'by_date orders descending by transacted_at' do
    old = create(:account_transaction, api_key: @api_key, exchange: @exchange, transacted_at: 2.days.ago)
    new_tx = create(:account_transaction, api_key: @api_key, exchange: @exchange, transacted_at: 1.hour.ago)

    results = AccountTransaction.by_date
    assert_equal [new_tx, old], results.to_a
  end

  test 'by_date_asc orders ascending by transacted_at' do
    old = create(:account_transaction, api_key: @api_key, exchange: @exchange, transacted_at: 2.days.ago)
    new_tx = create(:account_transaction, api_key: @api_key, exchange: @exchange, transacted_at: 1.hour.ago)

    results = AccountTransaction.by_date_asc
    assert_equal [old, new_tx], results.to_a
  end

  test 'in_date_range filters by from and to' do
    old = create(:account_transaction, api_key: @api_key, exchange: @exchange, transacted_at: 10.days.ago)
    mid = create(:account_transaction, api_key: @api_key, exchange: @exchange, transacted_at: 3.days.ago)
    recent = create(:account_transaction, api_key: @api_key, exchange: @exchange, transacted_at: 1.hour.ago)

    results = AccountTransaction.in_date_range(5.days.ago, 1.day.ago)
    assert_includes results, mid
    assert_not_includes results, old
    assert_not_includes results, recent
  end

  test 'in_date_range with only from' do
    old = create(:account_transaction, api_key: @api_key, exchange: @exchange, transacted_at: 10.days.ago)
    recent = create(:account_transaction, api_key: @api_key, exchange: @exchange, transacted_at: 1.hour.ago)

    results = AccountTransaction.in_date_range(5.days.ago, nil)
    assert_includes results, recent
    assert_not_includes results, old
  end

  test 'in_date_range with only to' do
    old = create(:account_transaction, api_key: @api_key, exchange: @exchange, transacted_at: 10.days.ago)
    recent = create(:account_transaction, api_key: @api_key, exchange: @exchange, transacted_at: 1.hour.ago)

    results = AccountTransaction.in_date_range(nil, 5.days.ago)
    assert_includes results, old
    assert_not_includes results, recent
  end

  test 'in_date_range with both nil returns all' do
    create(:account_transaction, api_key: @api_key, exchange: @exchange)
    assert_equal 1, AccountTransaction.in_date_range(nil, nil).count
  end

  # --- CSV Export ---

  test 'csv_headers returns correct columns' do
    expected = %w[date type base_currency base_amount quote_currency quote_amount fee_currency fee_amount exchange tx_id group_id description]
    assert_equal expected, AccountTransaction.csv_headers
  end

  test 'to_csv_row returns values in correct order' do
    at = create(:account_transaction,
                api_key: @api_key,
                exchange: @exchange,
                entry_type: :buy,
                base_currency: 'BTC',
                base_amount: 0.5,
                quote_currency: 'USD',
                quote_amount: 25_000.0,
                fee_currency: 'USD',
                fee_amount: 25.0,
                tx_id: 'order-abc',
                group_id: nil,
                description: 'DCA buy',
                transacted_at: Time.utc(2026, 3, 20, 10, 30, 0))

    row = at.to_csv_row
    assert_equal '2026-03-20T10:30:00Z', row[0]
    assert_equal 'buy', row[1]
    assert_equal 'BTC', row[2]
    assert_equal 0.5, row[3]
    assert_equal 'USD', row[4]
    assert_equal 25_000.0, row[5]
    assert_equal 'USD', row[6]
    assert_equal 25.0, row[7]
    assert_equal 'binance', row[8]
    assert_equal 'order-abc', row[9]
    assert_nil row[10]
    assert_equal 'DCA buy', row[11]
  end

  test 'to_csv generates valid CSV string' do
    create(:account_transaction,
           api_key: @api_key,
           exchange: @exchange,
           entry_type: :buy,
           base_currency: 'BTC',
           base_amount: 0.5,
           quote_currency: 'USD',
           quote_amount: 25_000.0,
           transacted_at: Time.utc(2026, 3, 20, 10, 30, 0))

    csv_string = AccountTransaction.to_csv(AccountTransaction.all)
    lines = csv_string.split("\n")
    assert_equal 2, lines.length
    assert_equal 'date,type,base_currency,base_amount,quote_currency,quote_amount,fee_currency,fee_amount,exchange,tx_id,group_id,description',
                 lines[0]
    assert_includes lines[1], '2026-03-20T10:30:00Z'
    assert_includes lines[1], 'buy'
    assert_includes lines[1], 'BTC'
  end

  test 'to_csv sorts ascending by date' do
    create(:account_transaction, api_key: @api_key, exchange: @exchange, transacted_at: Time.utc(2026, 3, 22), base_currency: 'ETH')
    create(:account_transaction, api_key: @api_key, exchange: @exchange, transacted_at: Time.utc(2026, 3, 20), base_currency: 'BTC')

    csv_string = AccountTransaction.to_csv(AccountTransaction.all)
    lines = csv_string.split("\n")
    assert_includes lines[1], 'BTC'
    assert_includes lines[2], 'ETH'
  end

  # --- Swap pairs ---

  test 'swap pairs share group_id' do
    swap_out = create(:account_transaction,
                      api_key: @api_key, exchange: @exchange,
                      entry_type: :swap_out, base_currency: 'ETH', base_amount: 1.0,
                      group_id: 'swap_trade-1', transacted_at: Time.current)
    swap_in = create(:account_transaction,
                     api_key: @api_key, exchange: @exchange,
                     entry_type: :swap_in, base_currency: 'BTC', base_amount: 0.05,
                     fee_currency: 'BTC', fee_amount: 0.0001,
                     group_id: 'swap_trade-1', transacted_at: Time.current)

    pair = AccountTransaction.where(group_id: 'swap_trade-1')
    assert_equal 2, pair.count
    assert_includes pair, swap_out
    assert_includes pair, swap_in
  end
end
