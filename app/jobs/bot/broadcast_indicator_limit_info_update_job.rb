class Bot::BroadcastIndicatorLimitInfoUpdateJob < ApplicationJob
  queue_as :default

  def perform(bot)
    bot.broadcast_indicator_limit_info_update
  end
end
