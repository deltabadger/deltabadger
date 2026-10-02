class Bot::FetchAndUpdateOpenOrdersJob < BotJob
  def perform(bot, update_missed_quote_amount: false, success_or_kill: false)
    # A retired venue has nothing left to poll (see Exchange::RETIRED_TYPES).
    return if bot.exchange&.retired?

    # TODO: The imported filter may be removed in the future once all users have re-exported
    # from the old app (which now correctly excludes unfilled orders from export)
    # This venue's orders only: a merged bot inherits rows placed elsewhere, and an id is only unique
    # per venue.
    external_order_ids = bot.transactions.waiting.where(exchange_id: bot.exchange_id)
                            .where.not("external_id LIKE 'imported_%'")
                            .pluck(:external_id)
    return if external_order_ids.empty?

    result = bot.get_orders(order_ids: external_order_ids)
    if result.failure?
      # Transient/rate-limit exchange-API failures bubble up as typed errors so they
      # funnel into ActionJob's retry_on (this job runs perform_now, inline in execute_action).
      raise Client::RateLimitedError, result.errors.to_sentence if bot.exchange.throttled_error?(result.errors)
      raise Client::TransientNetworkError, result.errors.to_sentence if bot.exchange.transient_failure?(result)

      raise "Failed to fetch orders #{external_order_ids.to_sentence}. Result: #{result.errors}"
    end

    young_missing_ids = []
    result.data[:missing].each do |missing_order_id|
      order = bot.transactions.find_by(exchange_id: bot.exchange_id, external_id: missing_order_id)
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

    result.data[:orders].each do |order_id, order_data|
      order = bot.transactions.find_by(exchange_id: bot.exchange_id, external_id: order_id)
      raise "Order #{order_id} not found" if order.nil?
      next unless order.submitted? && (order.open? || order.unknown?)

      case order_data[:status]
      when :open, :closed, :cancelled
        raise "Failed to update order #{order.external_id}" unless order.update_with_order_data(order_data)
      when :unknown
        raise "Order #{order.external_id} status is unknown."
      end
    end

    if young_missing_ids.any?
      # Decision #1: a non-authoritative exchange (no fill-recovery fallback) might be hiding an
      # undetected fill, so keep the loud guard. For an authoritative exchange (Kraken: QueryOrders
      # + TradesHistory) a still-missing order is confirmed never-executed — limit DCA self-heals
      # via missed_quote_amount, so DON'T wedge the bot or alarm the user; operator log only
      # (a SPIKE here is the real signal: regression / key problem).
      raise "Exchange omitted recent order(s): #{young_missing_ids.to_sentence}" unless bot.exchange.authoritative_missing_orders?

      Rails.logger.warn(
        "[orders-missing-from-source] bot_id=#{bot.id} exchange=#{bot.exchange.name_id} " \
        "order_ids=#{young_missing_ids.join(',')}"
      )
    end
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
