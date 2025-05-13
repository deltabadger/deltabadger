class Bot::BroadcastPnlUpdateJob < ApplicationJob
  queue_as :default

  def perform(bot)
    bot.broadcast_pnl_update
  end
end
