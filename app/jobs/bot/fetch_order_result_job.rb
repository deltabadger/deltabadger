class Bot::FetchOrderResultJob < BotJob
  def perform(bot_id, order_id)
    bot = Bot.find(bot_id)

    result = bot.fetch_order_result(order_id)
    raise StandardError, "Failed to fetch order #{order_id} result: #{result.errors}" unless result.success?
  end
end
