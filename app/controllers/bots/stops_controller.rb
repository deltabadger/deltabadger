class Bots::StopsController < ApplicationController
  include Bots::Botable

  before_action :authenticate_user!
  before_action :set_bot
  before_action :require_turbo_stream

  def update
    if @bot.stop
      if @bot.placement_intents.present? || @bot.transient_data['rust_placement'].present?
        flash.now[:notice] = Automation::Statusable::STOP_DISPATCH_NOTICE
      end
      return
    end

    flash.now[:alert] = @bot.errors.messages.values.flatten.to_sentence
    render turbo_stream: turbo_stream_prepend_flash, status: :unprocessable_entity
  end
end
