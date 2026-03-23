class Bots::Signals::AddApiKeysController < ApplicationController
  before_action :authenticate_user!

  def new
    @bot = current_user.bots.signal.new(sanitized_bot_config)

    if @bot.exchange_id.blank?
      redirect_to new_bots_signals_pick_exchange_path
    else
      @api_key = @bot.api_key
      if @api_key.key.present? && @api_key.secret.present? && !@api_key.correct?
        result = @api_key.get_validity
        @api_key.update_status!(result)
      end
      redirect_to new_bots_signals_pick_spendable_asset_path if @api_key.correct?
    end
  end

  def create
    @bot = current_user.bots.signal.new(sanitized_bot_config)
    @api_key = @bot.api_key
    @api_key.validate_credentials!(api_key_params)
    if @api_key.correct?
      render turbo_stream: turbo_stream_redirect(new_bots_signals_pick_spendable_asset_path)
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
end
