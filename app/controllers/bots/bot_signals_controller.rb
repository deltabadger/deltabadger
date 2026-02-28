class Bots::BotSignalsController < ApplicationController
  include Bots::Botable

  before_action :authenticate_user!
  before_action :set_bot

  def create
    @bot.bot_signals.create!(direction: :buy, amount: 100)
    render_settings
  end

  def update
    @signal = @bot.bot_signals.find(params[:id])
    @signal.update!(signal_params)
    if signal_params.key?(:enabled)
      render_settings
    else
      head :ok
    end
  end

  def destroy
    @signal = @bot.bot_signals.find(params[:id])
    if @bot.bot_signals.count <= 1
      flash.now[:alert] = t('errors.bots.signal_minimum')
      render turbo_stream: turbo_stream_prepend_flash, status: :unprocessable_entity
    else
      @signal.destroy!
      render_settings
    end
  end

  private

  def render_settings
    @bot.reload
    render turbo_stream: turbo_stream.replace('settings',
                                              partial: 'bots/signals/settings',
                                              locals: { bot: @bot })
  end

  def signal_params
    params.require(:bot_signal).permit(:direction, :amount, :amount_type, :enabled)
  end
end
