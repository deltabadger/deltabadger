class Setup::SeedAndSyncJob < ApplicationJob
  queue_as :low_priority

  def perform
    mark_sync_in_progress

    seed_exchanges
    sync_exchanges
    sync_assets_with_coingecko

    mark_sync_completed
  rescue StandardError => e
    mark_sync_failed(error_message: e.message)
  end

  private

  def seed_exchanges
    Rails.application.load_seed
  end

  def sync_exchanges
    if MarketDataSettings.deltabadger?
      sync_exchanges_from_deltabadger
    else
      sync_exchanges_from_coingecko
    end
  end

  def sync_exchanges_from_coingecko
    exchanges = Exchange.available.to_a
    exchanges.each_with_index do |exchange, index|
      # Skip async jobs during setup - we fetch asset data synchronously at the end
      exchange.sync_tickers_and_assets_with_external_data(skip_async_jobs: true)
      # Wait between exchanges to avoid CoinGecko rate limiting (30 req/min)
      sleep(65) if index < exchanges.length - 1
    rescue StandardError => e
      Rails.logger.warn "[Setup] Error syncing #{exchange.name}: #{e.message}"
    end
  end

  def sync_exchanges_from_deltabadger
    Exchange.available.each do |exchange|
      MarketData.sync_tickers!(exchange)
    rescue StandardError => e
      Rails.logger.warn "[Setup] Error syncing #{exchange.name}: #{e.message}"
    end
  end

  def sync_assets_with_coingecko
    if MarketDataSettings.deltabadger?
      sync_assets_from_deltabadger
    else
      sync_assets_from_coingecko
    end
  end

  def sync_assets_from_coingecko
    # Wait for rate limit to reset after exchange sync
    sleep(65)

    # Call existing job synchronously (skip its mark_sync_* methods by calling perform directly)
    job = Asset::FetchAllAssetsDataFromCoingeckoJob.new

    asset_ids = Asset.where(category: 'Cryptocurrency').pluck(:external_id).compact
    return if asset_ids.empty?

    result = job.send(:coingecko).get_coins_list_with_market_data(ids: asset_ids)
    if result.failure?
      Rails.logger.warn "[Setup] Failed to fetch CoinGecko data: #{result.errors.to_sentence}"
      return
    end

    Asset.where(category: 'Cryptocurrency').find_each do |asset|
      job.send(:sync_asset, asset, result.data.find { |coin| coin['id'] == asset.external_id })
    end
  end

  def sync_assets_from_deltabadger
    result = MarketData.sync_assets!
    Rails.logger.warn "[Setup] Failed to sync assets from market data service: #{result.errors.to_sentence}" if result.failure?

    result = MarketData.sync_indices!
    return unless result.failure?

    Rails.logger.warn "[Setup] Failed to sync indices from market data service: #{result.errors.to_sentence}"
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
    AppConfig.setup_sync_status = AppConfig::SYNC_STATUS_COMPLETED
    Rails.logger.error "[Setup] Sync failed: #{error_message}"
    broadcast_sync_completed
  end

  def broadcast_sync_completed
    Turbo::StreamsChannel.broadcast_remove_to(
      'settings_sync',
      target: 'flash-syncing'
    )
  end
end
