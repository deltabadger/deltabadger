class Bot::BroadcastMovingAverageLimitInfoUpdateJob < ApplicationJob
  queue_as :default

  def perform(bot)
    bot.broadcast_moving_average_limit_info_update
  end
end
