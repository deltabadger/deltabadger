# The one-time wash-sale question, asked at the first bot action that can sell. The answer is a
# property of the taxpayer, not of the bot that raised it — the bot is here only to decide whether
# the question is due at all, and to name the page the modal came up on.
class Bots::WashSalePromptsController < ApplicationController
  before_action :authenticate_user!
  before_action :set_bot

  def new; end

  # `update`, not `update!`: an unknown jurisdiction is a 422 that keeps the modal open carrying the
  # reason, not a 500. Nothing is enqueued and nothing is written on a refusal, so the question
  # comes back exactly as it was.
  def create
    case record_wash_sale_answer
    when nil then render_refusal(t('settings.wash_sale.prompt_missing'))
    when false then render_refusal(current_user.errors.full_messages.to_sentence)
    else render turbo_stream: turbo_stream_prepend_flash
    end
  end

  private

  def set_bot
    @bot = current_user.bots.find(params[:bot_id])
  end

  def render_refusal(message)
    current_user.reload
    flash.now[:alert] = message
    render turbo_stream: turbo_stream_prepend_flash, status: :unprocessable_entity
  end
end
