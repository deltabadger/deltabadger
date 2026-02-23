class Rules::WithdrawalsController < ApplicationController
  before_action :authenticate_user!
  before_action :set_rule, only: %i[update destroy confirm_destroy]

  def update
    status = update_params[:status]
    settings_params = update_params.except(:status)

    if settings_params.present?
      @rule.parse_params(settings_params)
      unless @rule.save
        flash.now[:alert] = @rule.errors.full_messages.to_sentence
        return render turbo_stream: turbo_stream_prepend_flash, status: :unprocessable_entity
      end
    end

    if status == 'scheduled'
      @rule.start
    elsif status == 'stopped'
      @rule.stop
    end

    render turbo_stream: turbo_stream_page_refresh
  rescue ActiveRecord::RecordInvalid => e
    flash.now[:alert] = e.record.errors.full_messages.to_sentence
    render turbo_stream: turbo_stream_prepend_flash, status: :unprocessable_entity
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

  def update_params
    params.require(:rules_withdrawal).permit(:status, :threshold_type, :max_fee_percentage, :min_amount)
  end
end
