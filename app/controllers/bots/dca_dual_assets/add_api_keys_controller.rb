class Bots::DcaDualAssets::AddApiKeysController < ApplicationController
  before_action :authenticate_user!

  def new
    @bot = current_user.bots.dca_dual_asset.new(sanitized_bot_config)

    if @bot.exchange_id.blank?
      redirect_to new_bots_dca_dual_assets_pick_exchange_path
    else
      @api_key = @bot.api_key
      # Only validate if key exists but isn't already confirmed correct
      if @api_key.key.present? && @api_key.secret.present? && !@api_key.correct?
        result = @api_key.get_validity
        @api_key.update_status!(result)
      end
      redirect_to after_api_key_path if @api_key.correct?
    end
  end

  def create
    @bot = current_user.bots.dca_dual_asset.new(sanitized_bot_config)
    if @bot.exchange_id.blank?
      redirect_to new_bots_dca_dual_assets_pick_exchange_path
      return
    end
    @api_key = @bot.api_key
    @api_key.validate_credentials!(api_key_params)
    if @api_key.correct?
      sync_alpaca_settings(@api_key) if @bot.exchange.is_a?(Exchanges::Alpaca)
      render turbo_stream: turbo_stream_redirect(after_api_key_path)
    elsif @api_key.incorrect?
      flash.now[:alert] = t('errors.incorrect_api_key_permissions')
      render :new, status: :unprocessable_entity
    else
      flash.now[:alert] = t('errors.api_key_permission_validation_failed')
      render :new, status: :unprocessable_entity
    end
  end

  private

  def api_key_params
    params.require(:api_key).permit(:key, :secret, :passphrase)
  end

  def stock_bot?
    @bot.base0_asset&.category == 'Stock' || @bot.base1_asset&.category == 'Stock'
  end

  def after_api_key_path
    # Stock bots get their quote asset auto-filled in set_stock_defaults, so
    # pick_spendable_asset#new will short-circuit and persist the bot.
    new_bots_dca_dual_assets_pick_spendable_asset_path
  end

  def sync_alpaca_settings(api_key)
    AppConfig.set('alpaca_api_key', api_key.key)
    AppConfig.set('alpaca_api_secret', api_key.secret)
    AppConfig.set('alpaca_mode', api_key.passphrase == 'live' ? 'live' : 'paper')
    Exchange::SyncAlpacaAssetsJob.perform_later
  end
end
