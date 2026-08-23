# One row per user per day: what the balances were worth, and how much money had come in from
# outside by then. The chart reads the series; the nightly balance sync appends today's row, so the
# history grows without a schedule of its own.
class PortfolioSnapshot < ApplicationRecord
  belongs_to :user

  scope :for_user, ->(user) { where(user_id: user.id) }

  # Balances are priced and stored in USD, so today's row is a sum. The invested figure is the
  # ledger's, computed here when the cache is cold — this runs inside a sync job, never in a
  # request. A user with neither balances nor transactions has no portfolio to record; one who
  # sold everything still has a day on the chart, at zero.
  def self.record!(user)
    balances = AccountBalance.for_user(user).nonzero.to_a
    return if balances.empty? && !AccountTransaction.for_user(user).exists?

    ledger = Tracker::Ledger.cached(user) || Tracker::Ledger.compute!(user)
    upsert({ user_id: user.id, date: Date.current,
             value_usd: balances.sum(0.to_d) { |balance| balance.usd_value.to_d },
             invested_usd: ledger.total_invested_usd, partial: partial?(user, balances) },
           unique_by: %i[user_id date], record_timestamps: true)
  end

  # What "we could not state this day in full" means: something held that we could not price, a
  # trading key that failed outright, or prices that lag the balances beside them — the quantities
  # are today's and the money is not.
  def self.partial?(user, balances)
    return true if balances.any? { |balance| balance.usd_value.to_d.zero? }
    return true if user.api_keys.any? { |key| key.sync_issue&.dig(:reason) == :failed }

    oldest_price = balances.filter_map(&:priced_at).min
    newest_sync = balances.filter_map(&:synced_at).max
    oldest_price.present? && newest_sync.present? && (newest_sync - oldest_price) > 5.minutes
  end
end
