class Bot::FetchAndCreateOrderJob < BotJob
  def perform(bot, order_id, update_missed_quote_amount: false)
    raise "Order #{order_id} already exists" if Transaction.exists?(external_id: order_id)

    result = fetch_order(bot, order_id)
    raise "Failed to fetch order #{order_id} result: #{result.errors}" if result.failure?

    order_data = result.data
    case order_data[:status]
    when :open, :closed
      ActiveRecord::Base.transaction do
        bot.create_submitted_order!(order_data)
        if update_missed_quote_amount
          missed_quote_amount = [0, bot.missed_quote_amount - order_data[:quote_amount_exec]].max
          bot.update!(missed_quote_amount: missed_quote_amount)
        end
      end
    when :unknown
      raise "Order #{order_id} status is unknown."
    end
  end

  private

  def fetch_order(bot, order_id, retries: 10, sleep_time: 0.5)
    return fetch_dry_order(order_id) if Rails.configuration.dry_run

    bot.with_api_key do
      retries.times do |i|
        result = bot.exchange.get_order(order_id: order_id)
        return result if result.success?

        Rails.logger.info "Order #{order_id} not fetched, retrying #{i + 1} of #{retries}..."
        sleep sleep_time
      end
      Result::Failure.new("Failed to fetch order #{order_id} after #{retries} attempts")
    end
  end

  def fetch_dry_order(order_id)
    order_data = Rails.cache.read(order_id)
    return Result::Failure.new("Dry order #{order_id} not found") if order_data.blank?

    Rails.cache.delete(order_id)
    Result::Success.new(order_data)
  end
end
