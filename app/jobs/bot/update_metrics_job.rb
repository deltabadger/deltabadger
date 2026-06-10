class Bot::UpdateMetricsJob < ApplicationJob
  queue_as :default

  limits_concurrency to: 1, key: ->(bot) { "bot_metrics_#{bot.id}" }, duration: 5.minutes

  def perform(bot)
    bot.metrics(force: true)
    # The 5-minute prices cache bakes in a copy of the base metrics; without forcing
    # it too, the show page and the broadcast below would serve pre-change balances
    # until the cache window rolls over.
    bot.metrics_with_current_prices(force: true)
    Bot::BroadcastMetricsUpdateJob.perform_later(bot)
  end
end
