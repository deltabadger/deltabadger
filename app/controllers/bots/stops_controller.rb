class Bots::StopsController < ApplicationController
  include Bots::Botable

  before_action :authenticate_user!
  before_action :set_bot

  def update
    return if @bot.stop

    flash.now[:alert] = @bot.errors.messages.values.flatten.to_sentence
    render turbo_stream: turbo_stream_prepend_flash, status: :unprocessable_entity
  end
end
