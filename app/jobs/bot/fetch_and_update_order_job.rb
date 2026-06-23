class Bot::FetchAndUpdateOrderJob < BotJob
  # Transient exchange-API failures (e.g. Kraken's HTTP-200 "EGeneral:Internal error"
  # / "EAPI:Invalid nonce") are retried with backoff instead of failing the job. This
  # job runs standalone async, so it needs its own retry_on (no exhaustion block — its
  # first arg is a Transaction; the durable row remains for the next open-orders sweep).
  retry_on Client::TransientNetworkError, wait: :polynomially_longer, attempts: 3
  # Rate limits retry on their own longer, escalating wait (re-trying too soon re-trips
  # Kraken's decaying counter). The durable row remains for the next sweep if exhausted.
  retry_on Client::RateLimitedError, wait: BotJob::RATE_LIMIT_WAIT, attempts: 4

  def perform(order, update_missed_quote_amount: false, success_or_kill: false)
    bot = order.bot
    result = bot.get_order(order_id: order.external_id)
    if result.failure?
      # A not_found Result may be resolved quietly (abandoned, or confirmed-never-executed on an
      # authoritative exchange); otherwise fall through to the typed-error / terminal-raise path.
      return if resolve_not_found(bot, order, result) == :handled

      raise Client::RateLimitedError, result.errors.to_sentence if bot.exchange.throttled_error?(result.errors)
      raise Client::TransientNetworkError, result.errors.to_sentence if bot.exchange.transient_error?(result.errors)

      raise "Failed to fetch order #{order.id}. Result: #{result.errors}"
    end

    calc_since = [bot.started_at, bot.settings_changed_at].compact.max
    order_data = result.data
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
  rescue StandardError => e
    return if success_or_kill

    raise e
  end

  private

  # Handle a not_found Result. Returns :handled when the caller should return quietly (the order
  # was abandoned, or it's a confirmed-never-executed young order on an authoritative exchange),
  # or :fall_through when the caller should proceed to the typed-error / terminal-raise path
  # (incl. any failure that is NOT a not_found signal).
  def resolve_not_found(bot, order, result)
    return :fall_through unless result.data.is_a?(Hash) && result.data[:not_found]

    case Bot::StaleOrderResolver.resolve(order)
    when :abandoned
      bot.log_activity('order_abandoned', details: { order_id: order.external_id })
      :handled
    when :too_young
      # An authoritative exchange (Kraken: QueryOrders + TradesHistory, Hyperliquid: userFills,
      # Bitvavo: paginated get_trades) has already exhausted its fill source inside get_order —
      # a still-missing order is confirmed never-executed, so resolving quietly (operator log,
      # no raise) is correct, exactly as Bot::FetchAndUpdateOpenOrdersJob does for an authoritative
      # young missing id. For a NON-authoritative exchange a dropped order may be a live order or a
      # real bug (wrong key, subaccount mismatch) → fall through and raise.
      return :fall_through unless bot.exchange.authoritative_missing_orders?

      Rails.logger.warn(
        "[orders-missing-from-source] bot_id=#{bot.id} exchange=#{bot.exchange.name_id} " \
        "order_ids=#{order.external_id}"
      )
      :handled
    end
  end
end
