class Bot::FetchAndUpdateOrderJob < BotJob
  def perform(order, update_missed_quote_amount: false)
    result = fetch_order(order)
    raise "Failed to fetch order #{order.external_id} result: #{result.errors}" if result.failure?

    order_data = result.data
    quote_amount_diff = order_data[:quote_amount_exec] - (order.quote_amount_exec || 0)
    case order_data[:status]
    when :open, :closed
      ActiveRecord::Base.transaction do
        order.update!(update_params(order_data))
        order.bot.update!(missed_quote_amount: [0, order.bot.missed_quote_amount - quote_amount_diff].max) if update_missed_quote_amount
      end
    when :unknown
      raise "Order #{order.external_id} status is unknown."
    end
  end

  private

  def fetch_order(order, retries: 10, sleep_time: 0.5)
    exchange = order.exchange
    api_key = order.bot.user.api_keys.trading.find_by(exchange: exchange)
    exchange.set_client(api_key: api_key)
    result = nil
    retries.times do |i|
      result = exchange.get_order(order_id: order.external_id)
      return result if result.success?

      Rails.logger.info "Order #{order.external_id} not fetched, retrying #{i + 1} of #{retries}..."
      sleep sleep_time
    end
    result || Result::Failure.new("Failed to fetch order #{order.external_id} after #{retries} attempts")
  end

  def update_params(order_data)
    {
      status: :submitted,
      external_status: order_data[:status],
      price: order_data[:price],
      amount: order_data[:amount],
      quote_amount: order_data[:quote_amount],
      base: order_data[:ticker].base_asset.symbol,
      quote: order_data[:ticker].quote_asset.symbol,
      side: order_data[:side],
      order_type: order_data[:order_type],
      amount_exec: order_data[:amount_exec],
      quote_amount_exec: order_data[:quote_amount_exec]
    }.compact
  end
end
