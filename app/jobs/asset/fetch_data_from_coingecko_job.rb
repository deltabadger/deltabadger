class Asset::FetchDataFromCoingeckoJob < ApplicationJob
  queue_as :low_priority

  def perform(asset, prefetched_data = nil)
    image_url_was = asset.image_url
    result = asset.sync_data_with_coingecko(prefetched_data: prefetched_data)
    raise StandardError, result.errors.to_sentence if result.failure?

    Asset::InferColorFromImageJob.perform_later(asset) if image_url_was != asset.image_url
  end
end
