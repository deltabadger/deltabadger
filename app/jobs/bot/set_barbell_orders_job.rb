class Bot::SetBarbellOrdersJob < BotJob
  def perform(bot_id)
    bot = Bot.find(bot_id)
    return unless can_set_orders?(bot)

    result = bot.set_barbell_orders
    raise StandardError, "Failed to set barbell orders: #{result.errors}" unless result.success?

    Bot::SetBarbellOrdersJob.set(wait: 1.public_send(bot.interval)).perform_later(bot_id)
  end

  private

  def can_set_orders?(bot)
    bot.working? || bot.pending?
  end
end
