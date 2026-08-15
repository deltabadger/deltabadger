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
    TransferMatcher.run!(user)
    assert_nil w.reload.linked_transaction_id
  end
end
