class ExchangeAsset < ApplicationRecord
  belongs_to :asset
  belongs_to :exchange

  validates :exchange_id, uniqueness: { scope: :asset_id }

  scope :available, -> { where(available: true) }

  include Undeletable
end
