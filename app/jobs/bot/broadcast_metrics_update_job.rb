class Bot::BroadcastMetricsUpdateJob < ApplicationJob
  queue_as :default

  def perform(bot)
    bot.broadcast_metrics_update
  end
end
