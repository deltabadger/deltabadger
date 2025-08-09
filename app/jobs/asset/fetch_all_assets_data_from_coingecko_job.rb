class Asset::FetchAllAssetsDataFromCoingeckoJob < ApplicationJob
  queue_as :low_priority

  def perform
    asset_ids = Asset.where(category: 'Cryptocurrency').pluck(:external_id)
    result = coingecko.get_coins_list_with_market_data(ids: asset_ids)
    raise result.errors.to_sentence if result.failure?

    Asset.where(category: 'Cryptocurrency').find_each do |asset|
      Asset::FetchDataFromCoingeckoJob.perform_later(
        asset,
        result.data.find { |coin| coin['id'] == asset.external_id }
      )
    end
  end

  private

  def coingecko
    @coingecko ||= Coingecko.new
  end
end
