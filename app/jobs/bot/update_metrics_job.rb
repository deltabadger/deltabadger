class Bot::UpdateMetricsJob < ApplicationJob
  queue_as :default

  limits_concurrency to: 1, key: ->(bot) { "bot_metrics_#{bot.id}" }, duration: 5.minutes

  def perform(bot)
    bot.metrics(force: true)
    Bot::BroadcastMetricsUpdateJob.perform_later(bot)
  end
end
