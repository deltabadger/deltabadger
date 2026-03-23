class Bots::AddApiKeysController < ApplicationController
  include Bots::Botable

  before_action :authenticate_user!
  before_action :set_bot

  def new
    @api_key = @bot.api_key
  end

  def create
    @api_key = @bot.api_key
    @api_key.validate_credentials!(api_key_params)
    if @api_key.correct?
      sync_alpaca_settings(@api_key) if @bot.exchange.is_a?(Exchanges::Alpaca)
      flash[:notice] = t('errors.bots.api_key_success')
      render turbo_stream: turbo_stream_page_refresh
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
    params.require(:api_key).permit(:key, :secret, :passphrase)
  end

  def sync_alpaca_settings(api_key)
    AppConfig.set('alpaca_api_key', api_key.key)
    AppConfig.set('alpaca_api_secret', api_key.secret)
    AppConfig.set('alpaca_mode', api_key.passphrase == 'live' ? 'live' : 'paper')
    Exchange::SyncAlpacaAssetsJob.perform_later
  end
end
