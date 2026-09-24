# The bot page's chart, loaded in its own lazy frame. Its cached payload grows with the bot's history
# and its number of assets, so reading and rendering it inline held the whole page back.
class Bots::ChartsController < ApplicationController
  before_action :authenticate_user!

  def show
    @bot = current_user.bots.find(params[:bot_id])
    @metrics = @bot.metrics_with_current_prices_and_candles_from_cache
    # The one place a bot page asks for a refresh: a cold chart, or a page whose own panels found
    # the prices cache cold (it says so in the frame URL rather than asking a second time).
    @refresh = @metrics.nil? || params[:metrics_missing].present?
  end
end
