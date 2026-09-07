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
end
