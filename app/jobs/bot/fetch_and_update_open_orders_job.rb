class Bot::FetchAndUpdateOpenOrdersJob < BotJob
  def perform(bot, update_missed_quote_amount: false, success_or_kill: false)
    orders = bot.transactions.submitted.open
    return if orders.empty?

    result = fetch_orders(bot, orders)
    raise "Failed to fetch orders #{orders.pluck(:id).to_sentence}. Result: #{result.errors}" if result.failure?

    calc_since = [bot.started_at, bot.settings_changed_at].compact.max
    result.data.each do |order_id, order_data|
      order = orders.find_by(external_id: order_id)
      raise "Order #{order_id} not found" if order.nil?

      quote_amount_diff = order_data[:quote_amount_exec] - (order.quote_amount_exec || 0)
      case order_data[:status]
      when :open, :closed
        raise "Failed to update order #{order.external_id}" unless order.update_from_order_data(order_data)

        if update_missed_quote_amount && order.created_at >= calc_since
          missed_quote_amount = [0, order.bot.missed_quote_amount - quote_amount_diff].max
          order.bot.update!(missed_quote_amount: missed_quote_amount)
        end
      when :unknown
        raise "Order #{order.external_id} status is unknown."
      end
    end
  rescue StandardError => e
    return if success_or_kill

    raise e
  end

  private

  def fetch_orders(bot, orders)
    bot.with_api_key do
      bot.exchange.get_orders(order_ids: orders.map(&:external_id))
    end
  end
end
