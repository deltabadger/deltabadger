class Index::SyncFromCoingeckoJob < ApplicationJob
  queue_as :low_priority

  def perform
    return unless AppConfig.coingecko_configured?

    @coingecko = Coingecko.new(api_key: AppConfig.coingecko_api_key)
    result = @coingecko.get_categories_with_market_data
    return if result.failure?

    # Build lookup of available asset external_ids
    available_asset_ids = Asset.where.not(external_id: nil).pluck(:external_id).to_set

    categories = result.data
    synced_ids = []

    categories.each do |category|
      next if category['id'].blank?
      next if Index::EXCLUDED_CATEGORY_IDS.include?(category['id'])
      next if category['content'].blank?

      # Fetch ALL coins for this category (up to 250)
      coins_result = fetch_category_coins(category['id'])
      sleep(3) # Rate limit: ~20 requests/minute to stay safe

      next if coins_result.nil?

      all_category_coins = coins_result[:coins]

      # Filter to coins that exist in our database
      valid_coins = all_category_coins.select { |coin_id| available_asset_ids.include?(coin_id) }

      # Skip if not enough coins in our database
      next if valid_coins.size < Index::ExchangeAvailability::MINIMUM_SUPPORTED_COINS

      # Take top 5 for display purposes
      top_coins_for_display = valid_coins.first(Index::ExchangeAvailability::TOP_COINS_COUNT)

      # Calculate available exchanges from live database using ALL valid coins
      available_exchanges = Index.calculate_available_exchanges(top_coins: valid_coins)

      # Skip indices with no exchange availability
      next if available_exchanges.empty?

      index = Index.find_or_initialize_by(
        external_id: category['id'],
        source: Index::SOURCE_COINGECKO
      )

      attrs = {
        name: strip_brackets(category['name']),
        description: category['content'],
        top_coins: top_coins_for_display,
        market_cap: category['market_cap'],
        available_exchanges: available_exchanges
      }

      # Set weight from WEIGHTED_CATEGORIES for new indices only
      if index.new_record?
        attrs[:weight] = Index::WEIGHTED_CATEGORIES[category['id']] || 0
      end

      index.update!(attrs)

      synced_ids << index.id
    rescue StandardError => e
      Rails.logger.warn "[Index Sync] Failed to sync category #{category['id']}: #{e.message}"
    end

    # Remove indices that no longer meet criteria
    Index.coingecko.where.not(id: synced_ids).delete_all

    Rails.logger.info "[Index Sync] Synced #{synced_ids.size} indices from CoinGecko"
  end

  private

  # Remove bracketed text from names, e.g. "Layer 1 (L1)" → "Layer 1"
  # Also handles brackets in the middle: "YZi Labs (Prev. Binance Labs) Portfolio" → "YZi Labs Portfolio"
  def strip_brackets(name)
    name&.gsub(/\s*\([^)]+\)/, '')&.gsub(/\s+/, ' ')&.strip
  end

  # Fetch all coins for a category (up to 250)
  # @return [Hash, nil] { coins: [...], total_count: N } or nil on failure
  def fetch_category_coins(category_id)
    result = @coingecko.get_coins_list_with_market_data(category: category_id, limit: 250)
    return nil if result.failure?

    coins = result.data
    {
      coins: coins.map { |coin| coin['id'] },
      total_count: coins.size
    }
  rescue StandardError => e
    Rails.logger.warn "[Index Sync] Failed to fetch coins for #{category_id}: #{e.message}"
    nil
  end
end
