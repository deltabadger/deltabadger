class Bot::FetchAndUpdateOpenOrdersJob < BotJob
  def perform(bot, update_missed_quote_amount: false, success_or_kill: false)
    # TODO: The imported filter may be removed in the future once all users have re-exported
    # from the old app (which now correctly excludes unfilled orders from export)
    external_order_ids = bot.transactions.waiting
                            .where.not("external_id LIKE 'imported_%'")
                            .pluck(:external_id)
    return if external_order_ids.empty?

    result = bot.get_orders(order_ids: external_order_ids)
    if result.failure?
      # Transient exchange-API failures bubble up as Client::TransientNetworkError so they
      # funnel into ActionJob's retry_on (this job runs perform_now, inline in execute_action).
      raise Client::TransientNetworkError, result.errors.to_sentence if bot.exchange.transient_error?(result.errors)

      raise "Failed to fetch orders #{external_order_ids.to_sentence}. Result: #{result.errors}"
    end

    young_missing_ids = []
    result.data[:missing].each do |missing_order_id|
      order = bot.transactions.find_by(external_id: missing_order_id)
      next if order.nil?

      case Bot::StaleOrderResolver.resolve(order)
      when :abandoned
        bot.log_activity('order_abandoned', details: { order_id: missing_order_id })
      when :too_young
        # Symmetric with the single-order job: a fresh order the exchange
        # silently dropped is almost certainly a real bug (wrong key,
        # subaccount mismatch). Record and raise after we've made any
        # legitimate progress on found/abandoned rows below.
        young_missing_ids << missing_order_id
      end
    end

    calc_since = [bot.started_at, bot.settings_changed_at].compact.max
    result.data[:orders].each do |order_id, order_data|
      order = bot.transactions.find_by(external_id: order_id)
      raise "Order #{order_id} not found" if order.nil?
      next unless order.submitted? && (order.open? || order.unknown?)

      quote_amount_diff = order_data[:quote_amount_exec] - (order.quote_amount_exec || 0)
      case order_data[:status]
      when :open, :closed, :cancelled
        raise "Failed to update order #{order.external_id}" unless order.update_with_order_data(order_data)

        if update_missed_quote_amount && order.created_at >= calc_since
          missed_quote_amount = [0, order.bot.missed_quote_amount - quote_amount_diff].max
          order.bot.update!(missed_quote_amount: missed_quote_amount)
        end
      when :unknown
        raise "Order #{order.external_id} status is unknown."
      end
    end

    raise "Exchange omitted recent order(s): #{young_missing_ids.to_sentence}" if young_missing_ids.any?
  rescue StandardError => e
    if success_or_kill
      Rails.logger.warn(
        'FetchAndUpdateOpenOrdersJob suppressed error ' \
        "bot_id=#{bot.id} exchange_id=#{bot.exchange_id} " \
        "order_ids=#{Array(external_order_ids).join(',')} error=#{e.message}"
      )
      return
    end

    raise e
  end
end
