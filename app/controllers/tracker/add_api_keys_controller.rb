class Tracker::AddApiKeysController < ApplicationController
  include RetiredExchangeGuard

  before_action :authenticate_user!

  def new
    if tracker_exchange.blank?
      redirect_to new_tracker_pick_exchange_path
      return
    end
    return if reject_retired_exchange(tracker_exchange, fallback: tracker_path)

    @exchange = tracker_exchange
    # Untyped (the + menu, a broken exchange chip): a bot key that reads fine needs nothing more.
    return redirect_to tracker_path if requested_key_type.nil? && healthy_trading_key

    @key_type = requested_key_type || 'read_only'
    @api_key = current_user.api_keys.find_or_initialize_by(exchange: @exchange, key_type: @key_type)
    if @api_key.key.present? && @api_key.secret.present? && !@api_key.correct?
      result = @api_key.get_validity
      @api_key.update_status!(result)
    end
    # An explicit choice always gets its form — the user asked to replace that key, even if it
    # checks out again. Untyped, a tracker key that already works needs nothing.
    return redirect_to tracker_path if requested_key_type.nil? && @api_key.correct? && !@api_key.missing_permission?

    render :reconnect if turbo_frame_request_id == 'modal'
  end

  def create
    @exchange = tracker_exchange
    if @exchange.blank?
      redirect_to new_tracker_pick_exchange_path
      return
    end
    return if reject_retired_exchange(@exchange, fallback: tracker_path)

    # Untyped submissions only ever write the tracker's own slot: the trading key is replaced only
    # when the user chose "Replace trading key".
    @key_type = requested_key_type || 'read_only'
    @api_key = current_user.api_keys.find_or_initialize_by(exchange: @exchange, key_type: @key_type)
    @api_key.validate_credentials!(api_key_params)

    if @api_key.correct?
      session.delete(:tracker_connect)
      AccountTransaction::SyncJob.perform_later(@api_key)
      # Updated here, before the redirect: #sync-warnings is data-turbo-permanent, so the visit
      # carries the old banner over and it would keep offering to replace the key just replaced.
      render turbo_stream: [
        turbo_stream.update('sync-warnings', partial: 'tracker/sync_warnings',
                                             locals: ApiKey.sync_warnings(current_user)),
        turbo_stream_redirect(tracker_path)
      ]
    elsif @api_key.incorrect?
      flash.now[:alert] = incorrect_api_key_message(@api_key)
      render turbo_stream: turbo_stream_prepend_flash, status: :unprocessable_entity
    else
      flash.now[:alert] = t('errors.api_key_permission_validation_failed')
      render turbo_stream: turbo_stream_prepend_flash, status: :unprocessable_entity
    end
  end

  private

  def api_key_params
    params.require(:api_key).permit(:key, :secret, :passphrase, :access_token, :rsa_signature_key, :rsa_encryption_key, :dh_param, :ibkr_realm)
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

  # The two slots the tracker may write: the bots' trading key (only on "Replace trading key") and
  # its own read-only key. Anything else in the parameter is ignored.
  KEY_TYPES = %w[trading read_only].freeze

  def requested_key_type
    params[:key_type].presence_in(KEY_TYPES)
  end

  # Trade permission contains read permission, so a bot key that reads is all the tracker needs —
  # unless it is missing a permission the sync needs.
  def healthy_trading_key
    key = current_user.api_keys.find_by(exchange: @exchange, key_type: :trading, status: :correct)
    key unless key&.missing_permission?
  end
end
