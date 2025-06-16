class Bots::CancelOrdersController < ApplicationController
  include Bots::Botable

  before_action :authenticate_user!
  before_action :set_bot
  before_action :set_transaction

  def destroy
    result = @transaction.cancel
    if result.success?
      flash.now[:notice] = t('bot.messages.order_cancelled')
      render turbo_stream: turbo_stream_prepend_flash
    else
      flash.now[:alert] = t('bot.messages.failed_to_cancel_order')
      render turbo_stream: turbo_stream_prepend_flash, status: :unprocessable_entity
    end
  end

  private

  def set_transaction
    @transaction = @bot.transactions.find(params[:id])
  end
end
