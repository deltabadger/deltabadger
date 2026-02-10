class Asset::FetchAllAssetsDataFromCoingeckoJob < ApplicationJob
  queue_as :low_priority

  def perform
    return unless MarketData.configured?

    result = MarketData.sync_assets!
    Rails.logger.warn "[MarketData] Failed to sync assets: #{result.errors.to_sentence}" if result.failure?
  rescue StandardError => e
    Rails.logger.warn "[MarketData] Error syncing assets: #{e.message}"
  end
end
