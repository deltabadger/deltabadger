# Before the ledger watermark was capped at the sync's start, a row dated ahead (an Alpaca split
# announced before its date) carried last_synced_at to that date. Every later sync asked from there
# less 25 h, so whatever the venue booked between the sync that stored the row and its date was never
# read. The cap stops new holes; this sends each damaged key back to that sync, and the next scheduled
# one refetches from there less its overlap. The rows it already holds come back as duplicates.
#
# The hole is recognisable after the date has passed too: a row stored more than the overlap before
# its own date, and a watermark past the moment it was stored. A row dated less than that ahead left
# no hole. The row may belong to any key of the same user and venue, or to none once its key is gone,
# so every key of that account goes back. A watermark still ahead of now with no such row to anchor on
# starts over, as a fresh key does.
class RewindLedgerWatermarksPastFutureRows < ActiveRecord::Migration[8.1]
  OVERLAP = 25.hours

  def up
    now = Time.current
    ApiKey.where.not(last_synced_at: nil).group_by { |key| [key.user_id, key.exchange_id] }
          .each do |(user_id, exchange_id), keys|
      # Rows dedupe per user and exchange, so the key that stored the row dated ahead may not be the
      # one whose watermark it carried, and a deleted key leaves its rows with no key at all.
      stored = AccountTransaction.where(user_id: user_id, exchange_id: exchange_id)
                                 .pluck(:transacted_at, :created_at)
                                 .filter_map { |at, created| created if at && at > created + OVERLAP }.min
      keys.each do |key|
        rewound = if stored && key.last_synced_at > stored then stored
                  elsif !stored && key.last_synced_at > now then nil
                  else next
                  end
        key.update_column(:last_synced_at, rewound)
      end
    end
  end

  def down
    # Nothing to restore: the watermark only moves back, and the next sync moves it forward again.
  end
end
