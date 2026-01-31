class Asset::FetchAllAssetsDataFromCoingeckoJob < ApplicationJob
  queue_as :low_priority

  def perform
    return unless AppConfig.coingecko_configured?

    mark_sync_in_progress

    asset_ids = Asset.where(category: 'Cryptocurrency').pluck(:external_id).compact
    result = coingecko.get_coins_list_with_market_data(ids: asset_ids)
    
    if result.failure?
      mark_sync_failed(error_message: result.errors.to_sentence)
      return
    end

    Asset.where(category: 'Cryptocurrency').find_each do |asset|
      sync_asset(asset, result.data.find { |coin| coin['id'] == asset.external_id })
    end

    mark_sync_completed
  rescue StandardError => e
    mark_sync_failed(error_message: e.message)
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

  def mark_sync_in_progress
    return unless AppConfig.setup_sync_pending?

    AppConfig.setup_sync_status = AppConfig::SYNC_STATUS_IN_PROGRESS
  end

  def mark_sync_completed
    return unless AppConfig.setup_sync_in_progress?

    AppConfig.setup_sync_status = AppConfig::SYNC_STATUS_COMPLETED
    broadcast_sync_completed
  end

  def mark_sync_failed(error_message:)
    AppConfig.setup_sync_status = AppConfig::SYNC_STATUS_PENDING
    broadcast_sync_failed(error_message)
  end

  def broadcast_sync_completed
    Turbo::StreamsChannel.broadcast_replace_to(
      "setup_sync",
      target: "setup-syncing-container",
      html: redirect_script
    )
  end

  def broadcast_sync_failed(error_message)
    Turbo::StreamsChannel.broadcast_replace_to(
      "setup_sync",
      target: "setup-syncing-container",
      html: error_html(error_message)
    )
  end

  def redirect_script
    "<script>window.location.href = '#{Rails.application.routes.url_helpers.bots_path}';</script>"
  end

  def error_html(error_message)
    <<~HTML
      <div class="setup-syncing__error">
        <p class="setup-syncing__error-message">#{error_message}</p>
        <a href="#{Rails.application.routes.url_helpers.setup_path}" class="button button--sky">
          #{I18n.t('setup.retry')}
        </a>
      </div>
    HTML
  end

  def coingecko
    @coingecko ||= Coingecko.new(api_key: AppConfig.coingecko_api_key)
  end
end
