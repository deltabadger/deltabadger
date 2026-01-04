class Asset::FetchAllAssetsDataFromCoingeckoJob < ApplicationJob
  queue_as :low_priority

  def perform
    mark_sync_in_progress

    asset_ids = Asset.where(category: 'Cryptocurrency').pluck(:external_id)
    result = coingecko.get_coins_list_with_market_data(ids: asset_ids)
    raise result.errors.to_sentence if result.failure?

    Asset.where(category: 'Cryptocurrency').find_each do |asset|
      sync_asset(asset, result.data.find { |coin| coin['id'] == asset.external_id })
    end

    mark_sync_completed
  end

  private

  def sync_asset(asset, prefetched_data)
    image_url_was = asset.image_url
    result = asset.sync_data_with_coingecko(prefetched_data: prefetched_data)
    raise StandardError, result.errors.to_sentence if result.failure?

    Asset::InferColorFromImageJob.perform_later(asset) if image_url_was != asset.image_url
  end

  def mark_sync_in_progress
    return unless AppConfig.setup_sync_pending?

    AppConfig.setup_sync_status = AppConfig::SYNC_STATUS_IN_PROGRESS
  end

  def mark_sync_completed
    return unless AppConfig.setup_sync_in_progress?

    AppConfig.setup_sync_status = AppConfig::SYNC_STATUS_COMPLETED
    broadcast_sync_completed
  end

  def broadcast_sync_completed
    Turbo::StreamsChannel.broadcast_replace_to(
      "setup_sync",
      target: "setup-syncing-container",
      html: redirect_script
    )
  end

  def redirect_script
    "<script>window.location.href = '#{Rails.application.routes.url_helpers.admin_root_path}';</script>"
  end

  def coingecko
    @coingecko ||= Coingecko.new
  end
end
