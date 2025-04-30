class Asset::FetchDataFromCoingeckoJob < ApplicationJob
  queue_as :default

  def perform(asset)
    result = asset.sync_data_with_coingecko
    raise StandardError, result.errors.to_sentence unless result.success?

    Asset::InferColorFromImageJob.perform_later(asset)
  end
end
