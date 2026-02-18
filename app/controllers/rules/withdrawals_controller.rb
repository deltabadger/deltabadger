class Rules::WithdrawalsController < ApplicationController
  before_action :authenticate_user!
  before_action :set_rule, only: %i[update destroy confirm_destroy]

  def update
    if @rule.update(update_params)
      render turbo_stream: turbo_stream_page_refresh
    else
      flash.now[:alert] = @rule.errors.full_messages.to_sentence
      render turbo_stream: turbo_stream_prepend_flash, status: :unprocessable_entity
    end
  end

  def confirm_destroy
    render layout: false
  end

  def destroy
    @rule.destroy
    render turbo_stream: turbo_stream_page_refresh
  end

  private

  def set_rule
    @rule = current_user.rules.find(params[:id])
  end

  def update_params
    params.require(:rules_withdrawal).permit(:status, :max_fee_percentage)
  end
end
