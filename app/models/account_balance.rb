class AccountBalance < ApplicationRecord
  belongs_to :user
  belongs_to :exchange
  belongs_to :asset

  validates :free, :locked, :synced_at, presence: true
  validates :asset_id, uniqueness: { scope: %i[user_id exchange_id] }

  scope :for_user, ->(user) { where(user_id: user.id) }
  scope :for_exchange, ->(exchange) { where(exchange_id: exchange.id) }
  scope :nonzero, -> { where('free + locked > 0') }
  scope :priced, -> { where('usd_value IS NOT NULL AND usd_value > 0') }
  scope :unpriced, -> { where('usd_value IS NULL OR usd_value <= 0') }
end
