class Bots::DcaIndexes::SetupCoingeckosController < ApplicationController
  before_action :authenticate_user!

  def new
    # Skip to pick index if CoinGecko is already configured and synced
    if AppConfig.coingecko_configured? && AppConfig.setup_sync_completed?
      session[:bot_config] ||= {}
      redirect_to new_bots_dca_indexes_pick_index_path
    end
  end

  def create
    api_key = params[:api_key]

    unless validate_coingecko_api_key(api_key)
      flash.now[:alert] = t('setup.invalid_coingecko_api_key')
      render turbo_stream: turbo_stream_prepend_flash, status: :unprocessable_entity
      return
    end

    AppConfig.coingecko_api_key = api_key
    session[:bot_config] ||= {}

    # Trigger sync and redirect to fullscreen syncing page
    AppConfig.setup_sync_status = AppConfig::SYNC_STATUS_PENDING
    Setup::SeedAndSyncJob.perform_later(source: 'settings', redirect_to: new_bots_dca_indexes_pick_index_path)
    render turbo_stream: turbo_stream_redirect(settings_syncing_path)
  end

  private

  def validate_coingecko_api_key(api_key)
    return false if api_key.blank?

    coingecko = Coingecko.new(api_key: api_key)
    result = coingecko.get_top_coins_by_market_cap(limit: 5)
    result.success?
  end
end
