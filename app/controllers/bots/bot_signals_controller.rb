class Bots::BotSignalsController < ApplicationController
  include Bots::Botable

  before_action :authenticate_user!
  before_action :set_bot
  # The controls are disabled on a running bot; a stale tab still posts. A rule edited under a
  # running bot changes what the next webhook call trades.
  before_action :refuse_while_running

  def create
    @bot.bot_signals.create!(direction: :buy, amount: 100)
    render_settings
  end

  def update
    @signal = @bot.bot_signals.find(params[:id])
    @signal.update!(signal_params)
    if signal_params.key?(:enabled)
      render_settings
    elsif (prompt = wash_sale_prompt_stream(@bot))
      # Switching a rule to sell answers `head :ok`, which carries nothing — and that is exactly the
      # action the question exists for.
      render turbo_stream: prompt
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

  def refuse_while_running
    return unless @bot.working?

    flash.now[:alert] = t('errors.bots.signal_locked_while_running')
    render turbo_stream: [turbo_stream_prepend_flash,
                          turbo_stream.replace('settings', partial: 'bots/signals/settings', locals: { bot: @bot })],
           status: :unprocessable_entity
  end

  def render_settings
    @bot.reload
    render turbo_stream: [turbo_stream.replace('settings',
                                               partial: 'bots/signals/settings',
                                               locals: { bot: @bot }),
                          wash_sale_prompt_stream(@bot)].compact
  end

  def signal_params
    params.require(:bot_signal).permit(:direction, :amount, :amount_type, :enabled)
  end
end
