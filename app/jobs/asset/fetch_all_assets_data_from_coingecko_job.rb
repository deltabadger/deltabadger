class Asset::FetchAllAssetsDataFromCoingeckoJob < ApplicationJob
  queue_as :low_priority

  def perform
    return unless MarketData.configured?

    asset_ids = Asset.where(category: 'Cryptocurrency').pluck(:external_id).compact
    result = coingecko.get_coins_list_with_market_data(ids: asset_ids)

    if result.failure?
      Rails.logger.warn "[CoinGecko] Failed to fetch asset data: #{result.errors.to_sentence}"
      return
    end

    Asset.where(category: 'Cryptocurrency').find_each do |asset|
      sync_asset(asset, result.data.find { |coin| coin['id'] == asset.external_id })
    end
  rescue StandardError => e
    Rails.logger.warn "[CoinGecko] Error fetching asset data: #{e.message}"
  end

  private

  def sync_asset(asset, prefetched_data)
    image_url_was = asset.image_url
    result = asset.sync_data_with_coingecko(prefetched_data: prefetched_data)
    if result.failure?
      Rails.logger.warn "[CoinGecko] Failed to sync asset #{asset.external_id}: #{result.errors.to_sentence}"
      return
    end

    Asset::InferColorFromImageJob.perform_later(asset) if image_url_was != asset.image_url
  rescue StandardError => e
    Rails.logger.warn "[CoinGecko] Error syncing asset #{asset.external_id}: #{e.message}"
  end

  def coingecko
    @coingecko ||= Coingecko.new(api_key: AppConfig.coingecko_api_key)
  end
end
