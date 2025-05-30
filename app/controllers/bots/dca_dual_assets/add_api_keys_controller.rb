class Bots::DcaDualAssets::AddApiKeysController < ApplicationController
  before_action :authenticate_user!

  def new
    @bot = current_user.bots.dca_dual_asset.new(session[:bot_config])

    if @bot.exchange_id.blank?
      redirect_to new_bots_dca_dual_assets_pick_exchange_path
    else
      @api_key = @bot.api_key
      @api_key.validate_key_permissions if @api_key.key.present? && @api_key.secret.present?
      redirect_to new_bots_dca_dual_assets_pick_spendable_asset_path if @api_key.correct?
    end
  end

  def create
    bot_config = session[:bot_config]
    @bot = current_user.bots.dca_dual_asset.new(bot_config)
    @api_key = @bot.api_key
    @api_key.key = api_key_params[:key]
    @api_key.secret = api_key_params[:secret]
    @api_key.validate_key_permissions
    if @api_key.correct? && @api_key.save
      flash.now[:notice] = t('errors.bots.api_key_success')
      redirect_to new_bots_dca_dual_assets_pick_spendable_asset_path
    else
      render :new, status: :unprocessable_entity
    end
  end

  private

  def api_key_params
    params.require(:api_key).permit(:key, :secret)
  end
end
