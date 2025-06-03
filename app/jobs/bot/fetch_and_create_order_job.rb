class Bot::FetchAndCreateOrderJob < BotJob
  def perform(bot, order_id, update_missed_quote_amount: false)
    return if Transaction.exists?(external_id: order_id)

    result = nil
    bot.with_api_key do
      result = bot.exchange.get_order(order_id: order_id)
    end
    order_data = result.data
    raise "Failed to fetch order #{order_id} result: #{result.errors}" if result.failure?
    raise "Order #{order_id} was not successful." if order_data[:status] == :unknown

    if order_data[:status] == :success
      bot.create_successful_order!(order_data)
      bot.update!(missed_quote_amount: [0, bot.missed_quote_amount - order_data[:quote_amount]].max) if update_missed_quote_amount
    elsif order_data[:status] == :failure
      bot.create_failed_order!(order_data)
    end
  end
end
