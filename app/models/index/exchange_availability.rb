module Index::ExchangeAvailability
  extend ActiveSupport::Concern

  MINIMUM_SUPPORTED_COINS = 3
  TOP_COINS_COUNT = 10 # Store more for flexibility in exchange picker (show top 5 available per exchange)
  DISPLAY_COINS_COUNT = 5

  class_methods do
    # Which exchanges can actually host this index, and with how many coins.
    #
    # Counted PER QUOTE and over tradable pairs only, because that is the question the quote picker
    # then asks (Bots::DcaIndex#available_assets_for_current_settings groups by quote_asset_id and
    # requires MINIMUM_SUPPORTED_COINS per quote). Counting listed pairs across every quote at once
    # offered venues whose quote list then came back empty — the same "the offer is not the answer"
    # divergence as the asset step.
    #
    # @param top_coins [Array<String>] CoinGecko coin ids — the coins actually exported for the
    #   venue, so availability can never be qualified on a wider set than the picker will use.
    # @return [Hash] Exchange types with coin counts, e.g. {"Exchanges::Binance" => 9}
    def calculate_available_exchanges(top_coins:)
      return {} if top_coins.blank?

      asset_ids = Asset.where(external_id: top_coins).pluck(:id)
      return {} if asset_ids.empty?

      result = {}
      Exchange.available.each do |exchange|
        # `|| 0`: max on no matching tickers is nil, and nil >= MINIMUM_SUPPORTED_COINS raises.
        matching_count = exchange.tickers.available.trading_enabled
                                 .where(base_asset_id: asset_ids)
                                 .group(:quote_asset_id).count.values.max || 0
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

  # Recalculate available exchanges from current database state. Qualifies each venue against the
  # coins actually exported for it, matching Index::SyncFromCoingeckoJob — otherwise a manual
  # refresh would quietly restore the wider, wrong universe.
  def refresh_available_exchanges!
    per_exchange = top_coins_by_exchange.presence
    new_availability =
      if per_exchange
        per_exchange.filter_map do |exchange_type, coins|
          count = self.class.calculate_available_exchanges(top_coins: coins)[exchange_type]
          [exchange_type, count] if count
        end.to_h
      else
        self.class.calculate_available_exchanges(top_coins: top_coins)
      end
    update!(available_exchanges: new_availability)
  end
end
