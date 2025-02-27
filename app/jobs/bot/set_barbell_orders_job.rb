class Bot::SetBarbellOrdersJob < BotJob
  def perform(bot_id)
    bot = Bot.find(bot_id)
    return unless can_set_orders?(bot)

    interval = bot.settings['interval']
    next_scheduled_orders_at = bot.next_scheduled_orders_at
    setting_orders_at = next_scheduled_orders_at - 1.public_send(interval)
    quote_amount = bot.next_scheduled_orders_quote_amount

    result = bot.set_barbell_orders(quote_amount)
    raise StandardError, "Failed to set barbell orders: #{result.errors}" unless result.success?

    bot.update!(last_scheduled_orders_at: setting_orders_at)

    Bot::SetBarbellOrdersJob.set(wait_until: next_scheduled_orders_at).perform_later(bot_id)
  end

  private

  def can_set_orders?(bot)
    bot.working? || bot.pending?
  end
end
