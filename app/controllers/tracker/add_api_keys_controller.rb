class Tracker::AddApiKeysController < ApplicationController
  before_action :authenticate_user!

  def new
    if tracker_exchange.blank?
      redirect_to new_tracker_pick_exchange_path
      return
    end

    @exchange = tracker_exchange
    @api_key = find_or_build_api_key
    if @api_key.key.present? && @api_key.secret.present? && !@api_key.correct?
      result = @api_key.get_validity
      @api_key.update_status!(result)
    end
    return redirect_to tracker_path if @api_key.correct?

    render :reconnect if turbo_frame_request_id == 'modal'
  end

  def create
    @exchange = tracker_exchange
    if @exchange.blank?
      redirect_to new_tracker_pick_exchange_path
      return
    end

    @api_key = find_or_build_api_key
    @api_key.validate_credentials!(api_key_params)

    if @api_key.correct?
      session.delete(:tracker_connect)
      AccountTransaction::SyncJob.perform_later(@api_key)
      render turbo_stream: turbo_stream_redirect(tracker_path)
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

  def tracker_exchange
    if params[:exchange_id].present?
      exchange = Exchange.find_by(id: params[:exchange_id])
      if exchange
        session[:tracker_connect] = { 'exchange_id' => exchange.id }
        return exchange
      end
    end
    exchange_id = session.dig('tracker_connect', 'exchange_id')
    Exchange.find_by(id: exchange_id) if exchange_id
  end

  def find_or_build_api_key
    current_user.api_keys.find_or_initialize_by(exchange: @exchange, key_type: :trading)
  end
end
