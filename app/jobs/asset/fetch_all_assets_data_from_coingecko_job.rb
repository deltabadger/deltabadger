class Asset::FetchAllAssetsDataFromCoingeckoJob < ApplicationJob
  queue_as :default

  def perform
    Asset.where(category: 'Cryptocurrency').find_each do |asset|
      Asset::FetchDataFromCoingeckoJob.perform_later(asset)
    end
  end
end
