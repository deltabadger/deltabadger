class Asset::FetchAllAssetsDataFromCoingeckoJob < ApplicationJob
  queue_as :low_priority
  limits_concurrency to: 1, key: -> { name }, on_conflict: :discard, duration: 1.hour

  def perform
    return unless MarketData.configured?

    result = MarketData.sync_assets!
    Rails.logger.warn "[MarketData] Failed to sync assets: #{result.errors.to_sentence}" if result.failure?
  rescue StandardError => e
    Rails.logger.warn "[MarketData] Error syncing assets: #{e.message}"
  end
end
