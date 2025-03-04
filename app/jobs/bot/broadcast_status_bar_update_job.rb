class Bot::BroadcastStatusBarUpdateJob < BotJob
  retry_on StandardError, wait: 0.1.seconds, attempts: 10

  def perform(bot_id, condition = nil)
    bot = Bot.find(bot_id)
    raise unless condition_met?(bot, condition)

    bot.broadcast_status_bar_update
  end

  private

  def condition_met?(bot, condition)
    return true if condition.blank?

    result = bot
    condition.split('.').each do |method_name|
      result = result.public_send(method_name)
    end
    result
  end
end
