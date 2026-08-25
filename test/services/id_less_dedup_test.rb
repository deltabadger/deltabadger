require 'test_helper'

# Recognising a row that carries no id of its own.
#
# The identity is (exchange, type, currency, amount, instant) — and it must NOT include the key that
# happens to be writing. A venue's history accumulates under whichever key was current at the time:
# a key gets replaced (its rows are nullified — `dependent: :nullify`), a reading key takes over from
# a rejected trading one, a rotation happens. Scope the check to one key and every one of those makes
# a file re-import the same history a second time — the ledger then holds twice the coins, and every
# P/L on the page goes silent because the balance and the ledger no longer agree.
#
# ponytail: two genuinely different SUB-ACCOUNTS on one venue that made an identical trade — same
# type, coin, amount, to the second, with no id from the venue — now collapse into one row. That is
# the cost, and it is the right way round: one swallowed dust row against a doubled tax history.
class IdLessDedupTest < ActiveSupport::TestCase
  setup do
    @user = create(:user)
    @binance = create(:binance_exchange)
    @old = create(:api_key, user: @user, exchange: @binance, key_type: :trading)
    @new = create(:api_key, user: @user, exchange: @binance, key_type: :read_only)
    @at = Time.utc(2021, 7, 1, 3, 42, 38)
  end

  def entry
    { entry_type: :buy, base_currency: 'LTC', base_amount: 0.06983.to_d,
      quote_currency: 'USDT', quote_amount: 10.to_d, fee_currency: nil, fee_amount: nil,
      tx_id: nil, group_id: nil, description: nil, transacted_at: @at, raw_data: {} }
  end

  def store(api_key, entries = [entry])
    AccountTransactionSync.new(api_key).store!(entries)
  end

  test 'a row already stored under another key on the same venue is recognised' do
    store(@old)

    result = store(@new)

    assert_equal 0, result[:imported]
    assert_equal 1, result[:duplicates]
    assert_equal 1, AccountTransaction.for_user(@user).count
  end

  # The case that produced it: the key that wrote the history was deleted, so its rows kept the
  # history and lost the key.
  test 'a row whose key was deleted is still recognised' do
    store(@old)
    @old.destroy
    assert_nil AccountTransaction.for_user(@user).sole.api_key_id, 'nullified with the key'

    assert_equal 0, store(@new)[:imported]
    assert_equal 1, AccountTransaction.for_user(@user).count
  end

  # A row the venue DID give an id for dedups on that id, which was never key-scoped.
  test 'an identified row is recognised whatever wrote it' do
    store(@old, [entry.merge(tx_id: 'LTCUSDT-1-2')])

    assert_equal 0, store(@new, [entry.merge(tx_id: 'LTCUSDT-1-2')])[:imported]
  end

  # The one that actually doubled a history. An exchange API timestamps to the MILLISECOND; its own
  # CSV export writes whole seconds. `22:58:17.200` and `22:58:17` are the same fill, and compared
  # exactly they are two — so every trade in the overlap lands a second time.
  test 'a whole second matches the millisecond it was truncated from' do
    store(@old, [entry.merge(tx_id: 'POLUSDC-1-2', transacted_at: @at + 0.2)])

    result = store(@new, [entry])

    assert_equal 0, result[:imported]
    assert_equal 1, AccountTransaction.for_user(@user).count
  end

  # Bucketed by second, so it reads the same from either side — whichever of the two arrives first.
  test 'the whole of one second is the same fill, and the next second is not' do
    store(@old, [entry.merge(tx_id: 'A', transacted_at: @at + 0.9)])

    assert_equal 0, store(@new, [entry])[:imported], 'still inside that second'
    assert_equal 1, store(@new, [entry.merge(transacted_at: @at + 1.second)])[:imported], 'the next second is its own'
  end

  test 'it matches whichever precision was stored first' do
    store(@old, [entry]) # whole seconds, from a file
    assert_equal 0, store(@new, [entry.merge(transacted_at: @at + 0.2)])[:imported] # ms, from an API
    assert_equal 1, AccountTransaction.for_user(@user).count
  end

  test 'another venue with the same shape is a different row' do
    kraken_key = create(:api_key, user: @user, exchange: create(:kraken_exchange))
    store(@old)

    assert_equal 1, store(kraken_key)[:imported]
    assert_equal 2, AccountTransaction.for_user(@user).count
  end

  test 'a genuinely different row still lands' do
    store(@old)

    assert_equal 1, store(@new, [entry.merge(base_amount: 0.5.to_d)])[:imported]
  end
end
