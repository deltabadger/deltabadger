class Bots::DcaIndexes::SetupCoingeckosController < ApplicationController
  before_action :authenticate_user!

  def new
    # Skip to pick index if market data is already configured
    return unless MarketData.configured?

    session[:bot_config] ||= {}
    redirect_to new_bots_dca_indexes_pick_index_path
  end

  def create
    api_key = params[:api_key]

    unless validate_coingecko_api_key(api_key)
      flash.now[:alert] = t('setup.invalid_coingecko_api_key')
      render turbo_stream: turbo_stream_prepend_flash, status: :unprocessable_entity
      return
    end

    AppConfig.coingecko_api_key = api_key
    AppConfig.market_data_provider = MarketDataSettings::PROVIDER_COINGECKO
    session[:bot_config] ||= {}

    Setup::SeedAndSyncJob.perform_later
    redirect_to new_bots_dca_indexes_pick_index_path
  end

  private

  def validate_coingecko_api_key(api_key)
    return false if api_key.blank?

    coingecko = Coingecko.new(api_key: api_key)
    result = coingecko.get_top_coins_by_market_cap(limit: 5)
    result.success?
  end
end
