module Index::ExchangeAvailability
  extend ActiveSupport::Concern

  MINIMUM_SUPPORTED_COINS = 3
  TOP_COINS_COUNT = 10 # Store more for flexibility in exchange picker (show top 5 available per exchange)
  DISPLAY_COINS_COUNT = 5

  class_methods do
    # Calculate which exchanges support enough coins from the index
    # @param top_coins [Array<String>] Array of CoinGecko coin IDs (external_ids)
    # @return [Hash] Exchange types with coin counts, e.g. {"Exchanges::Binance" => 9}
    def calculate_available_exchanges(top_coins:)
      return {} if top_coins.blank?

      result = {}

      # Get asset IDs for the top coins
      asset_ids_by_external_id = Asset.where(external_id: top_coins).pluck(:external_id, :id).to_h
      asset_ids = asset_ids_by_external_id.values

      return {} if asset_ids.empty?

      # Count how many coins each exchange supports
      Exchange.available.each do |exchange|
        matching_count = exchange.tickers.available.where(base_asset_id: asset_ids).count
        result[exchange.type] = matching_count if matching_count >= MINIMUM_SUPPORTED_COINS
      end

      result
    end
  end

  # Check if index is available on a specific exchange
  # @param exchange_type [String] Exchange class name, e.g. "Exchanges::Binance"
  def available_on_exchange?(exchange_type)
    available_exchanges.key?(exchange_type.to_s)
  end

  # Get Exchange records for all supported exchanges
  def supported_exchanges
    return Exchange.none if available_exchanges.blank?

    Exchange.where(type: available_exchanges.keys)
  end

  # Get the number of coins available on a specific exchange
  # @param exchange_type [String] Exchange class name
  # @return [Integer] Number of supported coins, or 0 if not available
  def coin_count_for_exchange(exchange_type)
    available_exchanges[exchange_type.to_s] || 0
  end

  # Recalculate available exchanges from current database state
  def refresh_available_exchanges!
    new_availability = self.class.calculate_available_exchanges(top_coins: top_coins)
    update!(available_exchanges: new_availability)
  end
end
