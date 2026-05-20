require 'test_helper'

class TransactionTest < ActiveSupport::TestCase
  # == waiting scope ==
  # "waiting" = a real order accepted by the exchange whose execution is not yet
  # confirmed: submitted rows with external_status open OR unknown. unknown rows
  # exist after Finding 1 (immediate persist) when the confirmation fetch has not
  # yet succeeded — they must still be treated as in-flight everywhere.

  test 'waiting scope returns submitted orders that are open or unknown' do
    bot = create(:dca_single_asset)
    open_txn = create(:transaction, bot: bot, status: :submitted, external_status: :open, external_id: 'o1')
    unknown_txn = create(:transaction, bot: bot, status: :submitted, external_status: :unknown, external_id: 'u1')
    closed_txn = create(:transaction, bot: bot, status: :submitted, external_status: :closed, external_id: 'c1')
    cancelled_txn = create(:transaction, bot: bot, status: :submitted, external_status: :cancelled, external_id: 'x1')

    waiting = bot.transactions.waiting

    assert_includes waiting, open_txn
    assert_includes waiting, unknown_txn
    assert_not_includes waiting, closed_txn
    assert_not_includes waiting, cancelled_txn
  end

  test 'waiting scope excludes failed and skipped rows even if external_status is set' do
    bot = create(:dca_single_asset)
    create(:transaction, bot: bot, status: :failed, external_status: :unknown, external_id: 'f1')
    create(:transaction, bot: bot, status: :skipped, external_status: :open, external_id: 's1')

    assert_empty bot.transactions.waiting
  end
end
