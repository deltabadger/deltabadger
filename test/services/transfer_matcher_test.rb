require 'test_helper'

class TransferMatcherTest < ActiveSupport::TestCase
  test 'pairs withdrawal with deposit within window and tolerance, idempotently' do
    user = create(:user)
    w = AccountTransaction.create!(user: user, exchange: create(:binance_exchange), entry_type: :withdrawal,
                                   base_currency: 'BTC', base_amount: 1, transacted_at: Time.utc(2024, 5, 1), tx_id: 'm1')
    d = AccountTransaction.create!(user: user, exchange: create(:kraken_exchange), entry_type: :deposit,
                                   base_currency: 'BTC', base_amount: '0.995'.to_d,
                                   transacted_at: Time.utc(2024, 5, 1, 4), tx_id: 'm2')
    TransferMatcher.run!(user)
    TransferMatcher.run!(user)
    assert_equal d.id, w.reload.linked_transaction_id
  end

  test 'does not pair across assets, beyond 72h, beyond 2 percent, or already-linked deposits' do
    user = create(:user)
    binance = create(:binance_exchange)
    kraken = create(:kraken_exchange)
    w = AccountTransaction.create!(user: user, exchange: binance, entry_type: :withdrawal,
                                   base_currency: 'BTC', base_amount: 1, transacted_at: Time.utc(2024, 5, 1), tx_id: 'm3')
    AccountTransaction.create!(user: user, exchange: kraken, entry_type: :deposit,
                               base_currency: 'ETH', base_amount: 1, transacted_at: Time.utc(2024, 5, 1, 1), tx_id: 'm4')
    AccountTransaction.create!(user: user, exchange: kraken, entry_type: :deposit,
                               base_currency: 'BTC', base_amount: '0.90'.to_d,
                               transacted_at: Time.utc(2024, 5, 1, 2), tx_id: 'm5')
    AccountTransaction.create!(user: user, exchange: kraken, entry_type: :deposit,
                               base_currency: 'BTC', base_amount: 1, transacted_at: Time.utc(2024, 5, 5), tx_id: 'm6')
    # A deposit another withdrawal already claimed. Without this, one deposit could relieve two
    # withdrawals' basis — and the unique index would blow the sync job up with RecordNotUnique.
    claimed = AccountTransaction.create!(user: user, exchange: kraken, entry_type: :deposit,
                                         base_currency: 'BTC', base_amount: '0.995'.to_d,
                                         transacted_at: Time.utc(2024, 5, 1, 3), tx_id: 'm7')
    AccountTransaction.create!(user: user, exchange: binance, entry_type: :withdrawal,
                               base_currency: 'BTC', base_amount: 1, transacted_at: Time.utc(2024, 5, 1),
                               tx_id: 'm8', linked_transaction_id: claimed.id)
    TransferMatcher.run!(user)
    assert_nil w.reload.linked_transaction_id
  end

  test 'never overwrites a manual link, even when a better candidate exists' do
    user = create(:user)
    binance = create(:binance_exchange)
    kraken = create(:kraken_exchange)
    withdrawal = AccountTransaction.create!(user: user, exchange: binance, entry_type: :withdrawal,
                                            base_currency: 'BTC', base_amount: 1,
                                            transacted_at: Time.utc(2024, 5, 1), tx_id: 'mm1')
    # Deliberately the worse match: further out in time and a bigger shrink than the decoy below.
    manual = AccountTransaction.create!(user: user, exchange: kraken, entry_type: :deposit,
                                        base_currency: 'BTC', base_amount: '0.98'.to_d,
                                        transacted_at: Time.utc(2024, 5, 2), tx_id: 'mm2')
    withdrawal.update!(linked_transaction_id: manual.id)
    AccountTransaction.create!(user: user, exchange: kraken, entry_type: :deposit,
                               base_currency: 'BTC', base_amount: '0.999'.to_d,
                               transacted_at: Time.utc(2024, 5, 1, 1), tx_id: 'mm3')

    TransferMatcher.run!(user)

    assert_equal manual.id, withdrawal.reload.linked_transaction_id
  end

  test 'never re-links a pair the user unlinked in the tracker' do
    user = create(:user)
    # The pair the matcher would happily match again: same asset, 1h apart, 0.5% shrink.
    withdrawal = AccountTransaction.create!(user: user, exchange: create(:binance_exchange), entry_type: :withdrawal,
                                            base_currency: 'BTC', base_amount: 1, transacted_at: Time.utc(2024, 5, 1),
                                            tx_id: 'r1', transfer_link_rejected: true)
    AccountTransaction.create!(user: user, exchange: create(:kraken_exchange), entry_type: :deposit,
                               base_currency: 'BTC', base_amount: '0.995'.to_d,
                               transacted_at: Time.utc(2024, 5, 1, 1), tx_id: 'r2')

    TransferMatcher.run!(user)

    assert_nil withdrawal.reload.linked_transaction_id
  end
end
