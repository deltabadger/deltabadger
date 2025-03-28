class ExchangeAsset < ApplicationRecord
  belongs_to :asset
  belongs_to :exchange

  # has_many :base_exchange_tickers, class_name: 'ExchangeTicker', foreign_key: 'base_asset_id'
  # has_many :quote_exchange_tickers, class_name: 'ExchangeTicker', foreign_key: 'quote_asset_id'

  validates :asset_id, uniqueness: { scope: :exchange_id }
end
