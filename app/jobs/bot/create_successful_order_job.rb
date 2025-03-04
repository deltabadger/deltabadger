class Bot::CreateSuccessfulOrderJob < BotJob
  def perform(bot, order_id)
    result = bot.exchange.get_order(order_id: order_id)
    raise StandardError, "Failed to fetch order #{order_id} result: #{result.errors}" unless result.success?
    raise StandardError, "Order #{order_id} was not successful." unless result.data[:status] == :success

    order_data = {
      base: result.data[:base],
      rate: result.data[:rate],
      amount: result.data[:amount]
    }

    bot.create_successful_order!(order_data, order_id)
  end
end
