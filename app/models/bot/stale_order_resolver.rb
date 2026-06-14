class Bot::StaleOrderResolver
  # Heuristic for "stop the polling spam, not an accounting claim." Governs orders absent
  # from EVERY fill source the exchange consults (Kraken: QueryOrders + TradesHistory),
  # i.e. never-executed / truly gone. NOT Kraken's actual retention — QueryOrders drops
  # terminal orders within hours, while TradesHistory retention is effectively unbounded;
  # this 14d figure is just how long we keep polling a missing order before giving up.
  STALE_ORDER_THRESHOLD = 14.days

  # Returns one of:
  #   :too_young — order is young enough that we keep polling; the caller decides whether
  #                a still-missing order warrants a loud failure (Bot::FetchAndUpdateOpenOrdersJob
  #                gates this on Exchange#authoritative_missing_orders?).
  #   :abandoned — order is old enough that we accept the exchange no longer
  #                tracks it, flip to terminal :abandoned, and stop polling.
  def self.resolve(order)
    return :too_young unless order.created_at < STALE_ORDER_THRESHOLD.ago

    order.update!(external_status: :abandoned)
    :abandoned
  end
end
