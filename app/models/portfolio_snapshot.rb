# One row per user per day: what the balances were worth, and how much money had come in from
# outside by then. The chart reads the series; the nightly balance sync appends today's row, so the
# history grows without a schedule of its own.
class PortfolioSnapshot < ApplicationRecord
  belongs_to :user

  CACHE_TTL = 2.days

  scope :for_user, ->(user) { where(user_id: user.id) }

  # Balances are priced and stored in USD, so today's row is a sum. The invested figure is the
  # ledger's, computed here when the cache is cold — this runs inside a sync job, never in a
  # request. A user with neither balances nor transactions has no portfolio to record; one who
  # sold everything still has a day on the chart, at zero.
  def self.record!(user)
    row = today_row(user)
    upsert(row, unique_by: %i[user_id date], record_timestamps: true) if row
  end

  # Today, from the balances and the ledger rather than from a price table that has not closed yet.
  # Stored for the whole portfolio; handed back for one venue, whose series is cached rather than
  # kept — see `series`.
  def self.today_row(user, exchange: nil)
    balances = AccountBalance.for_user(user).nonzero.then { |scope| exchange ? scope.for_exchange(exchange) : scope }.to_a
    return if balances.empty? && !AccountTransaction.for_user(user).exists?

    ledger = Tracker::Ledger.cached(user, exchange: exchange) || Tracker::Ledger.compute!(user, exchange: exchange)
    { user_id: user.id, date: Date.current,
      value_usd: balances.sum(0.to_d) { |balance| balance.usd_value.to_d },
      invested_usd: ledger.total_invested_usd, partial: partial?(user, balances) || ledger.incomplete }
  end

  # Which price generation this user's history was last swept at. A stored history is a reading of
  # the prices that existed when the sweep ran, and the nightly sync only ever appends TODAY — so a
  # price arriving for a day already written changes a figure nothing would otherwise revisit.
  PRICE_GENERATION_KEY = 'snapshot_price_generation'.freeze

  # Worth sweeping again: days that could not be valued, and prices that have arrived since the
  # sweep that could not value them. Both halves matter — the flag alone would ask again on every
  # page load for a day no price can fix (a negative balance from a venue whose window starts after
  # the funding deposit), and the generation alone would re-sweep a history with nothing wrong.
  def self.stale_prices?(user)
    return false unless for_user(user).exists?(partial: true)

    AppConfig.get(price_generation_key(user)).to_s != HistoricalPrice.generation.to_s
  end

  def self.mark_prices_swept!(user)
    AppConfig.set(price_generation_key(user), HistoricalPrice.generation.to_s)
  end

  # Per user: two people in one install do not share a sweep.
  def self.price_generation_key(user)
    "#{PRICE_GENERATION_KEY}_#{user.id}"
  end

  # The chart's series. The whole portfolio is a table — the nightly sync appends to it and every
  # page load reads it. One venue is a question asked occasionally, so it is swept on demand and
  # cached, the way a scoped ledger is: nil until a job has built it.
  def self.series(user, exchange: nil)
    return for_user(user).order(:date).to_a unless exchange

    Rails.cache.read(series_key(user, exchange))&.map { |row| new(row.except(:user_id)) }
  end

  def self.cache_series(user, exchange, rows)
    Rails.cache.write(series_key(user, exchange), rows, expires_in: CACHE_TTL)
  end

  # Follows the transactions, as the ledger's key does, plus the day itself: a series ends at
  # today, and tomorrow's answer is a different one.
  def self.series_key(user, exchange)
    scope = AccountTransaction.for_user(user).for_exchange(exchange)
    "tracker_history_v2_#{user.id}_#{exchange.id}_#{Date.current.iso8601}_" \
      "#{scope.maximum(:updated_at)&.utc&.iso8601(6)}_#{scope.count}"
  end

  # What "we could not state this day in full" means on the BALANCE side: something held that we
  # could not price, a trading key that failed outright, or prices that lag the balances beside them
  # — the quantities are today's and the money is not. The ledger answers for the other half: a
  # deposit it could not price is an invested figure that is understated, and a P/L read against an
  # understated cost is wrong in the flattering direction.
  def self.partial?(user, balances)
    return true if balances.any? { |balance| balance.usd_value.to_d.zero? }
    return true if user.api_keys.any? { |key| key.sync_issue&.dig(:reason) == :failed }

    oldest_price = balances.filter_map(&:priced_at).min
    newest_sync = balances.filter_map(&:synced_at).max
    oldest_price.present? && newest_sync.present? && (newest_sync - oldest_price) > 5.minutes
  end
end
