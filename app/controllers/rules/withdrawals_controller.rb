class Rules::WithdrawalsController < ApplicationController
  include ActionView::RecordIdentifier

  before_action :authenticate_user!
  before_action :set_rule, only: %i[update destroy confirm_destroy]

  def update
    status = update_params[:status]
    settings_params = update_params.except(:status)
    status_changed = (status == 'scheduled' && !@rule.scheduled?) || (status == 'stopped' && !@rule.stopped?)

    if settings_params.present?
      @rule.parse_params(settings_params)
      unless @rule.save
        flash.now[:alert] = @rule.errors.full_messages.to_sentence
        return render turbo_stream: [turbo_stream_prepend_flash, turbo_stream_page_refresh], status: :unprocessable_entity
      end
    end

    if status_changed
      streams = [turbo_stream_page_refresh]
      if status == 'scheduled'
        @rule.start
        flash.now[:success] = t('rules.tile.activated')
      else
        @rule.stop
        flash.now[:notice] = t('rules.tile.deactivated')
      end
      streams.unshift(turbo_stream_prepend_flash)
      render turbo_stream: streams
    else
      withdrawal_addresses = fetch_withdrawal_addresses(@rule)
      render turbo_stream: turbo_stream.replace(
        dom_id(@rule),
        partial: 'rules/rule_tile',
        locals: { rule: @rule, withdrawal_addresses: withdrawal_addresses }
      )
    end
  rescue ActiveRecord::RecordInvalid => e
    flash.now[:alert] = e.record.errors.full_messages.to_sentence
    render turbo_stream: [turbo_stream_prepend_flash, turbo_stream_page_refresh], status: :unprocessable_entity
  end

  def confirm_destroy
    render layout: false
  end

  def destroy
    @rule.delete
    render turbo_stream: turbo_stream_page_refresh
  end

  private

  def set_rule
    @rule = current_user.rules.find(params[:id])
  end

  def fetch_withdrawal_addresses(rule)
    return unless rule.stopped?

    api_key = current_user.api_keys.find_by(exchange: rule.exchange, key_type: :withdrawal)
    return unless api_key

    rule.exchange.set_client(api_key: api_key)
    rule.exchange.list_withdrawal_addresses(asset: rule.asset)
  end

  def update_params
    params.require(:rules_withdrawal).permit(:status, :threshold_type, :max_fee_percentage, :min_amount, :address)
  end
end
