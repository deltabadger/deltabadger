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

  # Raise the confirmed deadline and, with it, the effective one; never shorten either, and clear any
  # outstanding placement claim so no rollback can lower a deadline a real sale earned. Returns true
  # when the effective lock moved, which is what tells a caller this is news.
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
       'buy_locked_until = COALESCE(MAX(buy_locked_until, ?), ?), claim_token = NULL, source = ?',
       deadline, deadline, deadline, deadline, source]
    )
    was.nil? || deadline > was
  end
end
