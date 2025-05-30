class Bots::StartsController < ApplicationController
  before_action :authenticate_user!

  include Bots::Botable

  def edit
    render :edit_legacy if @bot.legacy?
  end

  def update
    return if @bot.start(start_fresh: Utilities::String.to_boolean(params[:start_fresh]))

    flash.now[:alert] = @bot.errors.messages.values.flatten.to_sentence
    render turbo_stream: turbo_stream_prepend_flash, status: :unprocessable_entity
  end
end
