class Rules::Withdrawals::AddApiKeysController < ApplicationController
  before_action :authenticate_user!

  def new
    @rule_config = session[:withdrawal_rule_config] || {}
    @asset = Asset.find_by(id: @rule_config['asset_id'])
    @exchange = Exchange.find_by(id: @rule_config['exchange_id'])

    if @exchange.blank?
      redirect_to new_rules_withdrawals_pick_exchange_path
      return
    end

    @api_key = current_user.api_keys.find_or_initialize_by(exchange: @exchange, key_type: :withdrawal)

    if @api_key.persisted? && @api_key.key.present? && @api_key.secret.present? && !@api_key.correct?
      result = @api_key.get_validity
      @api_key.update_status!(result)
    end

    redirect_to new_rules_withdrawals_confirm_settings_path if @api_key.correct?
  end

  def create
    @rule_config = session[:withdrawal_rule_config] || {}
    @asset = Asset.find_by(id: @rule_config['asset_id'])
    @exchange = Exchange.find(params[:exchange_id])
    @api_key = current_user.api_keys.find_or_initialize_by(exchange: @exchange, key_type: :withdrawal)
    @api_key.key = api_key_params[:key]
    @api_key.secret = api_key_params[:secret]
    @api_key.passphrase = api_key_params[:passphrase]

    result = @api_key.get_validity
    @api_key.update_status!(result)

    if @api_key.correct?
      render turbo_stream: turbo_stream_redirect(new_rules_withdrawals_confirm_settings_path)
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
