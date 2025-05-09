class Asset::FetchDataFromCoingeckoJob < ApplicationJob
  queue_as :default

  def perform(asset)
    image_url_was = asset.image_url
    result = asset.sync_data_with_coingecko
    raise StandardError, result.errors.to_sentence unless result.success?

    Asset::InferColorFromImageJob.perform_later(asset) if image_url_was != asset.image_url
  end
end
