class Bot::BroadcastStatusBarUpdateJob < BotJob
  def perform(bot)
    bot.broadcast_status_bar_update
  end
end
