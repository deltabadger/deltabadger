class Bot::BroadcastPriceLimitInfoUpdateJob < ApplicationJob
  queue_as :default

  def perform(bot)
    bot.broadcast_price_limit_info_update
  end
end
