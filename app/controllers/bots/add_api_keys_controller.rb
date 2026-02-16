class Bots::AddApiKeysController < ApplicationController
  include Bots::Botable

  before_action :authenticate_user!
  before_action :set_bot

  def new
    @api_key = @bot.api_key
  end

  def create
    @api_key = @bot.api_key
    @api_key.key = api_key_params[:key]
    @api_key.secret = api_key_params[:secret]
    @api_key.passphrase = api_key_params[:passphrase]
    result = @api_key.get_validity
    @api_key.update_status!(result)
    if @api_key.correct?
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
end
