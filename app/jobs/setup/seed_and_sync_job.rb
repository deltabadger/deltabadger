class Setup::SeedAndSyncJob < ApplicationJob
  queue_as :low_priority

  def perform(source: 'setup')
    @source = source
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
    exchanges = Exchange.available_for_new_bots.to_a
    exchanges.each_with_index do |exchange, index|
      # Skip async jobs during setup - we fetch asset data synchronously at the end
      exchange.sync_tickers_and_assets_with_external_data(skip_async_jobs: true)
      # Wait between exchanges to avoid CoinGecko rate limiting (30 req/min)
      sleep(65) if index < exchanges.length - 1
    rescue StandardError => e
      Rails.logger.warn "[Setup] Error syncing #{exchange.name}: #{e.message}"
    end
  end

  def sync_assets_with_coingecko
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
    if @source == 'settings'
      broadcast_settings_sync_completed
    else
      Turbo::StreamsChannel.broadcast_replace_to(
        "setup_sync",
        target: "setup-syncing-container",
        html: redirect_script
      )
    end
  end

  def broadcast_sync_failed(error_message)
    if @source == 'settings'
      broadcast_settings_sync_failed(error_message)
    else
      Turbo::StreamsChannel.broadcast_replace_to(
        "setup_sync",
        target: "setup-syncing-container",
        html: error_html(error_message)
      )
    end
  end

  def broadcast_settings_sync_completed
    Turbo::StreamsChannel.broadcast_remove_to("settings_sync", target: "flash-syncing")
  end

  def broadcast_settings_sync_failed(error_message)
    Turbo::StreamsChannel.broadcast_replace_to(
      "settings_sync",
      target: "flash-syncing",
      html: settings_error_html(error_message)
    )
  end

  def settings_error_html(error_message)
    <<~HTML
      <div id="flash-syncing" class="flash__message salert salert--danger" role="alert">
        #{error_message}
      </div>
    HTML
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
end
