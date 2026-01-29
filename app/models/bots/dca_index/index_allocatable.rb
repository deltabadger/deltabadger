module Bots::DcaIndex::IndexAllocatable
  extend ActiveSupport::Concern

  included do
    after_initialize :initialize_index_allocatable_settings
  end

  # Refresh the index composition by fetching top coins from CoinGecko
  # and matching them to available tickers on the exchange
  def refresh_index_composition
    Rails.logger.info("Refreshing index composition for bot #{id}")

    result = fetch_top_coins_with_allocations
    return result if result.failure?

    allocations = result.data
    update_bot_index_assets(allocations)

    Result::Success.new(allocations)
  end

  # Get current allocations for display
  def current_allocations
    bot_index_assets.in_index.includes(:asset, :ticker).order(target_allocation: :desc).map do |bia|
      {
        asset: bia.asset,
        ticker: bia.ticker,
        target_allocation: bia.target_allocation,
        current_allocation: bia.current_allocation,
        symbol: bia.asset.symbol
      }
    end
  end

  # Calculate allocations with flattening applied
  # @param market_caps [Hash] { asset_id => market_cap }
  # @return [Array<Hash>] allocations with { asset_id, ticker_id, weight }
  def calculate_allocations_with_flattening(coins_data)
    return [] if coins_data.empty?

    total_market_cap = coins_data.sum { |c| c[:market_cap].to_f }
    num_coins_in_index = coins_data.size
    equal_weight = 1.0 / num_coins_in_index

    coins_data.map do |coin|
      market_cap_weight = total_market_cap > 0 ? coin[:market_cap].to_f / total_market_cap : equal_weight
      # allocation_flattening: 0 = pure market cap, 1 = equal weight
      final_weight = market_cap_weight * (1 - allocation_flattening.to_f) + equal_weight * allocation_flattening.to_f

      {
        asset_id: coin[:asset_id],
        ticker_id: coin[:ticker_id],
        weight: final_weight,
        symbol: coin[:symbol],
        market_cap: coin[:market_cap]
      }
    end
  end

  private

  def initialize_index_allocatable_settings
    self.num_coins ||= 10
    self.allocation_flattening ||= 0.0
  end

  def fetch_top_coins_with_allocations
    coingecko = Coingecko.new(api_key: AppConfig.coingecko_api_key)
    # Fetch more coins than needed to account for ones not available on exchange
    fetch_limit = [num_coins.to_i * 3, 100].min

    result = if index_type == Bots::DcaIndex::INDEX_TYPE_CATEGORY && index_category_id.present?
               coingecko.get_top_coins_by_category(category: index_category_id, limit: fetch_limit)
             else
               coingecko.get_top_coins_by_market_cap(limit: fetch_limit)
             end
    return result if result.failure?

    top_coins = result.data
    available_tickers = exchange.tickers.available.where(quote_asset_id:).includes(:base_asset)

    # Build a map of CoinGecko ID to ticker
    ticker_by_coingecko_id = {}
    available_tickers.each do |ticker|
      next unless ticker.base_asset&.external_id.present?

      ticker_by_coingecko_id[ticker.base_asset.external_id] = ticker
    end

    # Match top coins to available tickers
    coins_data = []
    top_coins.each do |coin|
      break if coins_data.size >= num_coins.to_i

      ticker = ticker_by_coingecko_id[coin['id']]
      next unless ticker.present?

      coins_data << {
        asset_id: ticker.base_asset_id,
        ticker_id: ticker.id,
        symbol: ticker.base_asset.symbol,
        market_cap: coin['market_cap'].to_f,
        current_price: coin['current_price'].to_f,
        coingecko_id: coin['id']
      }
    end

    if coins_data.empty?
      return Result::Failure.new("No matching coins found on #{exchange.name} for the index")
    end

    allocations = calculate_allocations_with_flattening(coins_data)
    Result::Success.new(allocations)
  end

  def update_bot_index_assets(allocations)
    current_asset_ids = bot_index_assets.in_index.pluck(:asset_id)
    new_asset_ids = allocations.map { |a| a[:asset_id] }

    # Mark exited assets
    exited_asset_ids = current_asset_ids - new_asset_ids
    if exited_asset_ids.any?
      bot_index_assets.where(asset_id: exited_asset_ids, in_index: true).update_all(
        in_index: false,
        exited_at: Time.current
      )
    end

    # Upsert current allocations
    allocations.each do |alloc|
      bia = bot_index_assets.find_or_initialize_by(asset_id: alloc[:asset_id])
      bia.ticker_id = alloc[:ticker_id]
      bia.target_allocation = alloc[:weight]
      bia.in_index = true
      bia.entered_at ||= Time.current
      bia.exited_at = nil
      bia.save!
    end
  end
end
