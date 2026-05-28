class Bot::StaleOrderResolver
  # Heuristic for "stop the polling spam, not an accounting claim."
  # Matches Kraken's documented QueryOrders retention window for closed orders.
  STALE_ORDER_THRESHOLD = 14.days

  # Returns one of:
  #   :too_young — order is young enough that a missing exchange response is
  #                more likely a real bug (wrong key, subaccount mismatch); the
  #                caller should keep failing loudly.
  #   :abandoned — order is old enough that we accept the exchange no longer
  #                tracks it, flip to terminal :abandoned, and stop polling.
  def self.resolve(order)
    return :too_young unless order.created_at < STALE_ORDER_THRESHOLD.ago

    order.update!(external_status: :abandoned)
    :abandoned
  end
end
