class Bot::BroadcastPriceDropLimitInfoUpdateJob < ApplicationJob
  queue_as :default

  def perform(bot)
    bot.broadcast_price_drop_limit_info_update
  end
end
