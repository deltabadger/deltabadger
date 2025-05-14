class BroadcastsController < ApplicationController
  def metrics_update
    bot = Bot.find(params['bot_id'])
    return if bot.nil?

    Bot::BroadcastMetricsUpdateJob.perform_later(bot)
    head :ok
  end

  def pnl_update
    bots = Bot.where(id: params['bot_ids'])
    bots.each do |bot|
      Bot::BroadcastPnlUpdateJob.perform_later(bot)
    end
    head :ok
  end
end
