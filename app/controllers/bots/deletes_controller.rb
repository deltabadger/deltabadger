class Bots::DeletesController < ApplicationController
  include Bots::Botable

  before_action :authenticate_user!
  before_action :set_bot

  def edit; end

  def destroy
    if @bot.destroy
      flash[:notice] = t('errors.bots.destroy_success', bot_label: @bot.label)
      render turbo_stream: turbo_stream_redirect(bots_path)
    else
      flash.now[:alert] = @bot.errors.messages.values.flatten.to_sentence
      render turbo_stream: turbo_stream_prepend_flash, status: :unprocessable_entity
    end
  end
end
