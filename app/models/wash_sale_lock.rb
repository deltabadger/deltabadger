# One row per (taxpayer, asset): how long that asset is locked out of every buy leg after a sale
# that realised a loss. Deliberately NOT on bot_index_assets, where it started: those rows are
# destroyed with their bot, and a window the taxpayer is serving must not end because the bot that
# harvested the loss was deleted.
#
# Two deadlines, as before. `buy_locked_until` is the effective one and carries provisional
# placement-time locks that a proven non-placement rolls back; `confirmed_locked_until` is the part
# a fill confirmed and is only ever raised, so a rollback can never go below it. `claim_token` says
# which placement wrote the effective one — see Bot::WashSaleGuard#restore_buy_lock!.
class WashSaleLock < ApplicationRecord
  belongs_to :user
  belongs_to :asset

  scope :live, ->(now = Time.current) { where('buy_locked_until > ?', now) }

  def buy_locked?(now: Time.current)
    buy_locked_until.present? && buy_locked_until > now
  end

  # Raise the confirmed deadline and, with it, the effective one; never shorten either. Returns true
  # when the effective lock moved, which is what tells a caller this is news.
  #
  # The outstanding placement claim is left alone. It used to be cleared here, to stop a rollback
  # lowering a deadline a real sale earned — but restore_buy_lock! is already floored at
  # confirmed_locked_until, so the clear protected nothing and cost something: the ledger replaying
  # an OLD disposal would strip a live placement's token, and that placement could then never roll
  # its own provisional lock back when it provably failed.
  #
  # `from` is a Date. It must be — the window is added in DAYS, and a Time here would add seconds and
  # write a lock that has already expired.
  def self.confirm!(user:, asset_id:, from:, source: 'bot')
    days = user.wash_sale_days
    return false if days.zero?

    deadline = (from.to_date + days + 1).beginning_of_day
    lock = user.wash_sale_locks.find_or_create_by!(asset_id: asset_id)
    was = lock.buy_locked_until
    where(id: lock.id).update_all(
      ['confirmed_locked_until = COALESCE(MAX(confirmed_locked_until, ?), ?), ' \
       'buy_locked_until = COALESCE(MAX(buy_locked_until, ?), ?), source = ?',
       deadline, deadline, deadline, deadline, source]
    )
    was.nil? || deadline > was
  end
end
