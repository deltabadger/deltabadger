class Bots::DcaSingleAssetsController < ApplicationController
  before_action :authenticate_user!

  def create
    bot_config = session[:bot_config].deep_symbolize_keys
    @bot = current_user.bots.dca_single_asset.new(bot_config)
    @bot.set_missed_quote_amount
    if @bot.save && @bot.start(start_fresh: true)
      session[:bot_config] = nil
      render turbo_stream: turbo_stream_redirect(bot_path(@bot))
    else
      flash.now[:alert] = @bot.errors.messages.values.flatten.to_sentence
      render turbo_stream: turbo_stream_prepend_flash, status: :unprocessable_entity
    end
  end
end
