class Bots::DcaIndexes::AddApiKeysController < ApplicationController
  before_action :authenticate_user!
  before_action :require_market_data_configured

  def new
    @bot = current_user.bots.dca_index.new(session[:bot_config])

    if @bot.exchange_id.blank?
      redirect_to new_bots_dca_indexes_pick_exchange_path
    else
      @api_key = @bot.api_key
      # Only validate if key exists but isn't already confirmed correct
      if @api_key.key.present? && @api_key.secret.present? && !@api_key.correct?
        result = @api_key.get_validity
        @api_key.update_status!(result)
      end
      redirect_to new_bots_dca_indexes_pick_spendable_asset_path if @api_key.correct?
    end
  end

  def create
    bot_config = session[:bot_config]
    @bot = current_user.bots.dca_index.new(bot_config)
    @api_key = @bot.api_key
    @api_key.key = api_key_params[:key]
    @api_key.secret = api_key_params[:secret]
    @api_key.passphrase = api_key_params[:passphrase]
    result = @api_key.get_validity
    @api_key.update_status!(result)
    if @api_key.correct?
      render turbo_stream: turbo_stream_redirect(new_bots_dca_indexes_pick_spendable_asset_path)
    elsif @api_key.incorrect?
      flash.now[:alert] = t('errors.incorrect_api_key_permissions')
      render :new, status: :unprocessable_entity
    else
      flash.now[:alert] = t('errors.api_key_permission_validation_failed')
      render :new, status: :unprocessable_entity
    end
  end

  private

  def require_market_data_configured
    return if MarketData.configured?

    redirect_to new_bots_dca_indexes_setup_coingecko_path
  end

  def api_key_params
    params.require(:api_key).permit(:key, :secret, :passphrase)
  end
end
