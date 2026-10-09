require 'test_helper'
require Rails.root.join('db/migrate/20261009120000_rewind_ledger_watermarks_past_future_rows.rb')

# Before the watermark was capped at the sync's start, a row dated more than the 25 h overlap ahead
# (an Alpaca split announced before its date) carried last_synced_at to that date, and everything the
# venue booked in between was never read. The rewind sends each such key back to the sync that stored it.
class RewindLedgerWatermarksPastFutureRowsTest < ActiveSupport::TestCase
  NOW = Time.utc(2026, 10, 9, 12, 0, 0)

  def key_with(watermark, rows = [])
    key = create(:api_key, exchange: Exchange.find_by(type: 'Exchanges::Alpaca') || create(:alpaca_exchange),
                           last_synced_at: watermark)
    rows.each_with_index do |(transacted_at, created_at), i|
      create(:account_transaction, api_key: key, tx_id: "t#{key.id}-#{i}", transacted_at: transacted_at,
                                   created_at: created_at)
    end
    key
  end

  def migrate = ActiveRecord::Migration.suppress_messages { RewindLedgerWatermarksPastFutureRows.new.up }

  test 'a watermark carried past rows never read goes back to the sync that stored the row dated ahead' do
    travel_to NOW do
      stored = Time.utc(2026, 10, 3, 8, 8, 50)
      # The split, read on 3 Oct and dated 5 Oct, has since passed: the watermark is no longer in the future,
      # but 3–4 Oct were never fetched.
      healed = key_with(Time.utc(2026, 10, 5), [[Time.utc(2026, 10, 1), Time.utc(2026, 10, 1, 2)],
                                                [Time.utc(2026, 10, 5), stored]])
      # Still ahead: the split is dated next week.
      ahead = key_with(Time.utc(2026, 10, 15), [[Time.utc(2026, 10, 15), Time.utc(2026, 10, 8, 2)]])
      # Ahead of now with no rows of its own to say where the hole starts: the whole history again.
      empty = key_with(Time.utc(2026, 10, 15))
      # A row dated less than the overlap ahead left no hole; nor does a watermark already behind the row's sync.
      near = key_with(Time.utc(2026, 10, 9, 6), [[Time.utc(2026, 10, 9, 6), Time.utc(2026, 10, 8, 23)]])
      behind = key_with(Time.utc(2026, 10, 2), [[Time.utc(2026, 10, 15), Time.utc(2026, 10, 3)]])
      ordinary = key_with(Time.utc(2026, 10, 8), [[Time.utc(2026, 10, 8), Time.utc(2026, 10, 8, 2)]])
      never = key_with(nil)

      migrate

      assert_equal stored, healed.reload.last_synced_at
      assert_equal Time.utc(2026, 10, 8, 2), ahead.reload.last_synced_at
      assert_nil empty.reload.last_synced_at
      assert_equal Time.utc(2026, 10, 9, 6), near.reload.last_synced_at
      assert_equal Time.utc(2026, 10, 2), behind.reload.last_synced_at
      assert_equal Time.utc(2026, 10, 8), ordinary.reload.last_synced_at
      assert_nil never.reload.last_synced_at

      before = ApiKey.order(:id).pluck(:id, :last_synced_at, :updated_at)
      migrate
      assert_equal before, ApiKey.order(:id).pluck(:id, :last_synced_at, :updated_at), 'a second run changes nothing'
    end
  end
  # Rows dedupe per user and exchange, so a second key of the same account skipped storing the split
  # the first one stored, yet its watermark went to the split's date all the same. Deleting the first
  # key leaves the row behind with no key.
  test 'every key of the account goes back, whichever key stored the row dated ahead' do
    travel_to NOW do
      stored = Time.utc(2026, 10, 3, 8, 8, 50)
      original = key_with(Time.utc(2026, 10, 5), [[Time.utc(2026, 10, 5), stored]])
      replacement = create(:api_key, user: original.user, exchange: original.exchange, key_type: :read_only,
                                     last_synced_at: Time.utc(2026, 10, 5))
      superseded = key_with(Time.utc(2026, 10, 5), [[Time.utc(2026, 10, 5), stored]])
      successor = create(:api_key, user: superseded.user, exchange: superseded.exchange, key_type: :read_only,
                                   last_synced_at: Time.utc(2026, 10, 6))
      deleted = key_with(Time.utc(2026, 10, 5), [[Time.utc(2026, 10, 5), stored]])
      survivor = create(:api_key, user: deleted.user, exchange: deleted.exchange, key_type: :read_only,
                                  last_synced_at: Time.utc(2026, 10, 7))
      deleted.destroy!
      assert_nil AccountTransaction.find_by(user_id: deleted.user_id).api_key_id
      # Another account on the same venue holds no such row and keeps its watermark.
      bystander = key_with(Time.utc(2026, 10, 7), [[Time.utc(2026, 10, 7), Time.utc(2026, 10, 7, 1)]])

      migrate

      assert_equal stored, original.reload.last_synced_at
      assert_equal stored, replacement.reload.last_synced_at
      assert_equal stored, superseded.reload.last_synced_at
      assert_equal stored, successor.reload.last_synced_at
      assert_equal stored, survivor.reload.last_synced_at
      assert_equal Time.utc(2026, 10, 7), bystander.reload.last_synced_at

      before = ApiKey.order(:id).pluck(:id, :last_synced_at, :updated_at)
      migrate
      assert_equal before, ApiKey.order(:id).pluck(:id, :last_synced_at, :updated_at), 'a second run changes nothing'
    end
  end
end
