class Bot::BroadcastStatusBarUpdateAfterScheduledOrderJob < BotJob
  def perform(bot)
    50.times do
      break if bot.next_action_job_at.present?

      sleep 0.1
    end

    bot.broadcast_status_bar_update
  end
end
