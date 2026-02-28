class Bots::SignalsController < ApplicationController
  before_action :authenticate_user!

  def create
    bot_config = session[:bot_config]
    signals_config = bot_config.delete('signals') || [{ 'direction' => 'buy', 'amount' => 100 }]
    @bot = current_user.bots.signal.new(bot_config.deep_symbolize_keys)
    signals_config.each do |wh|
      @bot.bot_signals.build(direction: wh['direction'], amount: wh['amount'], enabled: wh.fetch('enabled', true),
                             amount_type: wh.fetch('amount_type', 'fixed'))
    end
    if @bot.save && @bot.start(start_fresh: true)
      session[:bot_config] = nil
      render turbo_stream: turbo_stream_redirect(bot_path(@bot))
    else
      flash.now[:alert] = @bot.errors.messages.values.flatten.to_sentence
      render turbo_stream: turbo_stream_prepend_flash, status: :unprocessable_entity
    end
  end
end
