class BotIndexAsset < ApplicationRecord
  belongs_to :bot
  belongs_to :asset
  belongs_to :ticker

  scope :in_index, -> { where(in_index: true) }
  scope :exited, -> { where(in_index: false) }

  validates :target_allocation, numericality: { greater_than_or_equal_to: 0, less_than_or_equal_to: 1 }, allow_nil: true
  validates :current_allocation, numericality: { greater_than_or_equal_to: 0, less_than_or_equal_to: 1 }, allow_nil: true

  # Locked out of every buy leg by the wash-sale guard (Bot::WashSaleGuard) until this passes.
  def buy_locked?(now: Time.current)
    buy_locked_until.present? && buy_locked_until > now
  end
end
