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
      flash.now[:alert] = error_message_for(result)
      render turbo_stream: turbo_stream_prepend_flash, status: :unprocessable_entity
    end
  end

  private

  def error_message_for(result)
    if invalid_api_key_error?(result)
      t('bot.messages.failed_to_cancel_order_invalid_api_key', exchange_name: @bot.exchange.name)
    else
      t('bot.messages.failed_to_cancel_order')
    end
  end

  def invalid_api_key_error?(result)
    return false unless @bot.exchange.respond_to?(:known_errors)

    invalid_key_messages = @bot.exchange.known_errors[:invalid_key] || []
    result.errors.any? { |error| invalid_key_messages.include?(error) }
  end

  def set_transaction
    @transaction = @bot.transactions.find(params[:id])
  end
end
