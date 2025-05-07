class BotUpdatesChannel < ApplicationCable::Channel
  def subscribed
    stream_from "user_#{current_user.id}", :bot_updates
  end

  def broadcast_metrics_update(params)
    bot = Bot.find(params['bot_id'])
    return if bot.nil?

    Bot::BroadcastMetricsUpdateJob.perform_later(bot)
  end

  def broadcast_pnl_update(params)
    bots = Bot.where(id: params['bot_ids'])
    bots.each do |bot|
      Bot::BroadcastPnlUpdateJob.perform_later(bot)
    end
  end
end
