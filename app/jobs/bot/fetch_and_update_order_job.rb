class Bot::FetchAndUpdateOrderJob < BotJob
  def perform(order, update_missed_quote_amount: false, success_or_kill: false)
    bot = order.bot
    result = bot.get_order(order_id: order.external_id)
    raise "Failed to fetch order #{order.id}. Result: #{result.errors}" if result.failure?

    calc_since = [bot.started_at, bot.settings_changed_at].compact.max
    order_data = result.data
    quote_amount_diff = order_data[:quote_amount_exec] - (order.quote_amount_exec || 0)
    case order_data[:status]
    when :open, :closed
      raise "Failed to update order #{order.external_id}" unless order.update_with_order_data(order_data)

      if update_missed_quote_amount && order.created_at >= calc_since
        missed_quote_amount = [0, order.bot.missed_quote_amount - quote_amount_diff].max
        order.bot.update!(missed_quote_amount: missed_quote_amount)
      end
    when :unknown
      raise "Order #{order.external_id} status is unknown."
    end
  rescue StandardError => e
    return if success_or_kill

    raise e
  end
end
