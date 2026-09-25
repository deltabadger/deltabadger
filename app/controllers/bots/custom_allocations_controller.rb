# "Custom allocation" in an index bot's menu: the bot keeps its current members at their current
# weights as a portfolio (Bot::IndexSwitch.customize!).
class Bots::CustomAllocationsController < ApplicationController
  include Bots::Botable

  before_action :authenticate_user!
  before_action :set_bot

  def create
    return head :not_found unless @bot.dca_index?

    bot = Bot::IndexSwitch.customize!(@bot)
    render turbo_stream: turbo_stream_redirect(bot_path(bot))
  rescue Bot::IndexSwitch::Refused => e
    flash.now[:alert] = e.message
    render turbo_stream: turbo_stream_prepend_flash, status: :unprocessable_entity
  end
end
