class Bot::FetchAndUpdateOrderJob < BotJob
  # Transient exchange-API failures (e.g. Kraken's HTTP-200 "EGeneral:Internal error"
  # / "EAPI:Invalid nonce") are retried with backoff instead of failing the job. This
  # job runs standalone async, so it needs its own retry_on (no exhaustion block — its
  # first arg is a Transaction; the durable row remains for the next open-orders sweep).
  retry_on Client::TransientNetworkError, wait: :polynomially_longer, attempts: 3
  # Rate limits retry on their own longer, escalating wait (re-trying too soon re-trips
  # Kraken's decaying counter). The durable row remains for the next sweep if exhausted.
  retry_on Client::RateLimitedError, wait: BotJob::RATE_LIMIT_WAIT, attempts: 4

  # An order the venue had accepted but not filled when it was asked (Alpaca answers :open for a
  # just-accepted or mid-fill order). Asked again for every bot, not only signal bots: a signal bot
  # has no tick, a stopped bot's sale (Sell all, a rebalance) has none either, and a weekly bot's
  # next tick is a week away — the row stayed "waiting", so the sold shares still read as held.
  # Read-only — a retry re-reads an order, it never places one — and bounded: the open row stays
  # for the sweeps and the page's own refresh if it outlives the retries.
  class OrderStillOpen < StandardError; end

  retry_on OrderStillOpen, wait: :polynomially_longer, attempts: 8 do |job, _error|
    order = job.arguments.first
    Rails.logger.warn("[order-still-open] bot_id=#{order.bot_id} order_id=#{order.external_id} polling stopped")
  end

  def perform(order, update_missed_quote_amount: false, success_or_kill: false)
    # Keyed off the ORDER's venue, not the bot's: bot.exchange is mutable, so once a stranded bot
    # is moved to a live exchange, a job still queued for the old order would otherwise ask the new
    # venue about an order id it has never seen. transactions.exchange_id records where the order
    # was actually placed. Nothing is fetchable from a venue that no longer exists, and the failure
    # path below raises rather than degrading, so this has to no-op.
    return if order.exchange&.retired?

    bot = order.bot
    # Same rule for a bot that is simply on another venue than the order: a merged bot inherits its
    # sources' rows, and a bot can be moved. The merge refuses a source with orders still resting
    # elsewhere, so a poll landing here is one queued for a row another sweep has since settled.
    if order.exchange_id != bot.exchange_id
      Rails.logger.info("FetchAndUpdateOrderJob: order #{order.id} was placed on exchange #{order.exchange_id}, " \
                        "bot #{bot.id} is on #{bot.exchange_id}; nothing to ask")
      return
    end

    result = bot.get_order(order_id: order.external_id)
    if result.failure?
      # A not_found Result may be resolved quietly (abandoned, or confirmed-never-executed on an
      # authoritative exchange); otherwise fall through to the typed-error / terminal-raise path.
      return if resolve_not_found(bot, order, result) == :handled

      raise Client::RateLimitedError, result.errors.to_sentence if bot.exchange.throttled_error?(result.errors)
      raise Client::TransientNetworkError, result.errors.to_sentence if bot.exchange.transient_failure?(result)

      raise "Failed to fetch order #{order.id}. Result: #{result.errors}"
    end

    # The venue took its time answering; the row may have changed hands meanwhile (a merge moves it
    # to the bot it now belongs to). Read it back so the callbacks the update fires — the broadcast,
    # the metrics refresh — address that bot, not the one this job was handed.
    order.reload

    order_data = result.data
    case order_data[:status]
    when :open, :closed, :cancelled
      raise "Failed to update order #{order.external_id}" unless order.update_with_order_data(order_data)

      raise OrderStillOpen if order_data[:status] == :open
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
