class ExchangeTicker < ApplicationRecord
  belongs_to :exchange
  belongs_to :base_asset, class_name: 'Asset'
  belongs_to :quote_asset, class_name: 'Asset'

  validates :exchange_id, uniqueness: { scope: %i[base_asset_id quote_asset_id] }
  validate :exchange_matches_assets

  def get_price
    result = exchange.get_tickers_prices
    return result unless result.success?
    return Result::Failure.new("Price not found for #{ticker} on #{exchange.name}") unless result.data.key?(ticker)

    Result::Success.new(result.data[ticker])
  end

  def get_minimum_base_size_in_quote
    result = get_price
    return result unless result.success?

    Result::Success.new(minimum_base_size * result.data)
  end

  private

  def exchange_matches_assets
    return if base_asset.exchanges.include?(exchange) && quote_asset.exchanges.include?(exchange)

    errors.add(:exchange, 'must match the exchange of base and quote assets')
  end
end
