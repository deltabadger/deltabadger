# Before the ledger watermark was capped at the sync's start, a row dated ahead (an Alpaca split
# announced before its date) carried last_synced_at to that date. Every later sync asked from there
# less 25 h, so whatever the venue booked between the sync that stored the row and its date was never
# read. The cap stops new holes; this sends each damaged key back to that sync, and the next scheduled
# one refetches from there less its overlap. The rows it already holds come back as duplicates.
#
# The hole is recognisable after the date has passed too: a row stored more than the overlap before
# its own date, and a watermark past the moment it was stored. A row dated less than that ahead left
# no hole. A watermark still ahead of now with no such row of its own to anchor on starts over, as a
# fresh key does.
class RewindLedgerWatermarksPastFutureRows < ActiveRecord::Migration[8.1]
  OVERLAP = 25.hours

  def up
    now = Time.current
    ApiKey.where.not(last_synced_at: nil).find_each do |key|
      stored = AccountTransaction.where(api_key_id: key.id).pluck(:transacted_at, :created_at)
                                 .filter_map { |at, created| created if at && at > created + OVERLAP }.min
      rewound = if stored && key.last_synced_at > stored then stored
                elsif !stored && key.last_synced_at > now then nil
                else next
                end
      key.update_column(:last_synced_at, rewound)
    end
  end

  def down
    # Nothing to restore: the watermark only moves back, and the next sync moves it forward again.
  end
end
