class Bots::StartsController < ApplicationController
  include Bots::Botable

  before_action :authenticate_user!
  before_action :set_bot

  def edit
    render :edit_legacy if @bot.legacy?
  end

  def update
    return if @bot.start(start_fresh: Utilities::String.to_boolean(params[:start_fresh]))

    flash.now[:alert] = @bot.errors.messages.values.flatten.to_sentence
    render turbo_stream: turbo_stream_prepend_flash, status: :unprocessable_entity
  end
end
