class Bots::DcaDualAssets::AddApiKeysController < ApplicationController
  before_action :authenticate_user!

  def new
    @bot = current_user.bots.dca_dual_asset.new(session[:bot_config])

    if @bot.exchange_id.blank?
      redirect_to new_bots_dca_dual_assets_pick_exchange_path
    else
      @api_key = @bot.api_key
      result = @api_key.get_validity
      @api_key.update_status!(result) if @api_key.key.present? && @api_key.secret.present?
      redirect_to new_bots_dca_dual_assets_pick_spendable_asset_path if @api_key.correct?
    end
  end

  def create
    bot_config = session[:bot_config]
    @bot = current_user.bots.dca_dual_asset.new(bot_config)
    @api_key = @bot.api_key
    @api_key.key = api_key_params[:key]
    @api_key.secret = api_key_params[:secret]
    result = @api_key.get_validity
    @api_key.update_status!(result)
    if @api_key.correct?
      redirect_to new_bots_dca_dual_assets_pick_spendable_asset_path
    elsif @api_key.incorrect?
      flash.now[:alert] = t('errors.incorrect_api_key_permissions')
      render turbo_stream: turbo_stream_prepend_flash, status: :unprocessable_entity
    else
      flash.now[:alert] = t('errors.api_key_permission_validation_failed')
      render turbo_stream: turbo_stream_prepend_flash, status: :unprocessable_entity
    end
  end

  private

  def api_key_params
    params.require(:api_key).permit(:key, :secret)
  end
end
