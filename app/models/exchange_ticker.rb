class ExchangeTicker < ApplicationRecord
  belongs_to :exchange
  belongs_to :base_asset, class_name: 'Asset'
  belongs_to :quote_asset, class_name: 'Asset'

  validates :exchange_id, uniqueness: { scope: %i[base_asset_id quote_asset_id] }
  validate :exchange_matches_assets

  private

  def exchange_matches_assets
    return if base_asset.exchanges.include?(exchange) && quote_asset.exchanges.include?(exchange)

    errors.add(:exchange, 'must match the exchange of base and quote assets')
  end
end
