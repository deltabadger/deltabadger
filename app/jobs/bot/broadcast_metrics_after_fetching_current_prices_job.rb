class Bot::BroadcastMetricsAfterFetchingCurrentPricesJob < ApplicationJob
  queue_as :default

  def perform(bot)
    puts "Performing Bot::BroadcastMetricsAfterFetchingCurrentPricesJob for bot #{bot.id}"
    bot.broadcast_metrics_update
  end
end
