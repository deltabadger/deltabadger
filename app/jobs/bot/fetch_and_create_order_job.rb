class Bot::FetchAndCreateOrderJob < BotJob
  def perform(bot, order_id)
    return if Transaction.exists?(external_id: order_id)

    result = bot.exchange.get_order(order_id: order_id)
    raise StandardError, "Failed to fetch order #{order_id} result: #{result.errors}" unless result.success?
    raise StandardError, "Order #{order_id} was not successful." if result.data[:status] == :unknown

    bot.create_successful_order!(result.data) if result.data[:status] == :success
    bot.create_failed_order!(result.data) if result.data[:status] == :failure
  end
end
