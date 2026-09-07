class BotIndexAsset < ApplicationRecord
  belongs_to :bot
  belongs_to :asset
  belongs_to :ticker

  scope :in_index, -> { where(in_index: true) }
  scope :exited, -> { where(in_index: false) }

  validates :target_allocation, numericality: { greater_than_or_equal_to: 0, less_than_or_equal_to: 1 }, allow_nil: true
  validates :current_allocation, numericality: { greater_than_or_equal_to: 0, less_than_or_equal_to: 1 }, allow_nil: true
end
