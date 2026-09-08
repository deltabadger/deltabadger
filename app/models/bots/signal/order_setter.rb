# What a rule does when its webhook is called: one market order, sized by the rule, recorded like
# any other bot's — the shape of Bots::DcaSingleAsset::OrderSetter with the schedule taken out.
#
# There is no next interval to carry a lost signal to, so every outcome leaves a row: a submitted,
# skipped or failed transaction, or a placement_ambiguous line. Failures are classified by WHERE
# they happen, because that is what decides whether money moved:
#   - before the placement call (sizing, price and balance reads) → a failed row; nothing reached
#     the venue;
#   - inside the placement call → ambiguous; the request may have gone out, and an unknown outcome
#     is never retried and never written down as a failure;
#   - after acceptance → the submitted row stays whatever else goes wrong; the venue holds the order.
module Bots::Signal::OrderSetter
  extend ActiveSupport::Concern

  include Bot::OrderSetter
  include Bot::OrderCreator

  def execute_signal(signal)
    # Before signal_order_data: that one reads the venue and can record a failure the user is emailed
    # about, all for a signal that is about to be skipped anyway. The asset is known from the bot's
    # own ticker, so nothing it produces is needed here.
    if signal.buy? && user.locked_asset_ids.include?(ticker.base_asset_id)
      Rails.logger.info("execute_signal bot=#{id} event=order_wash_sale_locked base=#{ticker.base}")
      return
    end

    order_data = signal_order_data(signal)
    return if order_data.nil?

    amount_info = calculate_best_amount_info(order_data)
    if amount_info[:below_minimum_amount]
      Rails.logger.info("execute_signal bot=#{id} event=order_skipped #{order_log_fields(order_data)}")
      log_activity('order_skipped', level: :warning, details: order_log_details(order_data))
      record_outcome { create_skipped_order!(order_data) }
      return
    end

    order_id = place_signal_order(signal, order_data, amount_info)
    track_signal_order(order_data, order_id) if order_id
  end

  # A failure nothing reached the venue for: one row, and one email decision, whatever raised it.
  def record_signal_failure(signal, errors, order_data: nil)
    order_data ||= { ticker: ticker, side: signal.direction.to_sym, order_type: :market_order }
    Rails.logger.error(
      "execute_signal bot=#{id} event=order_failed #{order_log_fields(order_data)} errors=#{errors.to_sentence}"
    )
    notify = !signal_failing? # read BEFORE the row that would answer "yes"
    record_outcome { create_failed_order!(order_data.merge(error_messages: errors)) }
    notify_signal_failure(errors) if notify
  end

  private

  # Sizing. Every read here is idempotent and retried in place; what still fails becomes a failed
  # row. Sells are sized in base at the bid, never beyond what is free — the wallet, not the bot's
  # own history, is the ceiling, exactly as for a selling DCA bot.
  def signal_order_data(signal)
    side = signal.direction.to_sym
    price = signal_price(side)
    amount, quote_amount = signal_size(signal, side, price)
    { ticker: ticker, price: price, amount: amount, quote_amount: quote_amount, side: side, order_type: :market_order }
  rescue StandardError => e
    record_signal_failure(signal, [e.message])
    nil
  end

  # The price a market order would execute at, from the venue: on the venues that emulate a
  # market order by crossing the spread (Hyperliquid, Gemini) that is the touch plus the cross,
  # and sizing off the raw touch would spend more than the rule says.
  def signal_price(side)
    result = exchange.with_transient_retry { exchange.market_price_for(ticker: ticker, side: side) }
    raise result.errors.to_sentence if result.failure?

    result.data
  end

  def signal_size(signal, side, price)
    if side == :buy
      quote_amount = signal.percentage? ? spendable_quote_balance * signal.amount / 100 : signal.amount
      [quote_amount / price, quote_amount]
    else
      free = free_base_balance
      amount = signal.percentage? ? free * signal.amount / 100 : [signal.amount / price, free].min
      [amount, amount * price]
    end
  end

  # What can actually be spent — on a margin venue that is buying power, and the venue decides.
  def spendable_quote_balance
    exchange.spendable_balance(signal_balance(quote_asset_id), tickers: [ticker]).to_d
  end

  def free_base_balance
    signal_balance(base_asset_id)[:free].to_d
  end

  def signal_balance(asset_id)
    result = exchange.with_transient_retry { get_balance(asset_id: asset_id) }
    raise result.errors.to_sentence if result.failure?

    result.data
  end

  # Placement. A -1021 is a pre-trade rejection — the signed timestamp failed, so nothing reached
  # the book — and is placed once more with a fresh clock. Anything else that did not get a clean
  # answer (Exchange#ambiguous_placement_error?: a network failure handed back as a Result, a
  # gateway 5xx, an acceptance with no order id) is a request whose fate is unknown, and gets the
  # same answer as a raised one: a warning line, no row, no email, no retry.
  def place_signal_order(signal, order_data, amount_info)
    Rails.logger.info("execute_signal bot=#{id} event=order_creating #{order_log_fields(order_data)}")
    result = create_order(order_data, amount_info)
    result = create_order(order_data, amount_info) if result.failure? && exchange.placement_transient_error?(result.errors)

    if result.success?
      order_id = result.data[:order_id]
      return order_id if exchange.acknowledged_order_id?(order_id)

      return ambiguous_placement(order_data, 'the venue accepted the order but returned no order id')
    end

    if exchange.ambiguous_placement_error?(result)
      ambiguous_placement(order_data, result.errors.to_sentence)
    else
      record_signal_failure(signal, result.errors, order_data: order_data)
    end
    nil
  rescue StandardError => e
    ambiguous_placement(order_data, e.message)
  end

  def ambiguous_placement(order_data, message)
    Rails.logger.warn(
      "execute_signal bot=#{id} event=placement_ambiguous #{order_log_fields(order_data)} error=#{message}"
    )
    log_activity('placement_ambiguous', level: :warning, details: order_log_details(order_data).merge(error: message))
    nil
  end

  # From here the venue holds the order. Nothing that goes wrong on our side may be written down as
  # a failed order; the submitted row and its exchange id are what lets the fill be confirmed. A row
  # that could not be written, or a confirmation that could not be queued, is said out loud with
  # the exchange order id, so it can be reconciled by hand.
  def track_signal_order(order_data, order_id)
    Rails.logger.info("execute_signal bot=#{id} event=order_accepted order_id=#{order_id} #{order_log_fields(order_data)}")
    transaction = persist_accepted_order!(order_data, order_id)
    raise 'the confirmation job was not enqueued' unless Bot::FetchAndUpdateOrderJob.perform_later(transaction)
  rescue StandardError => e
    Rails.logger.error("execute_signal bot=#{id} event=accepted_order_untracked order_id=#{order_id} error=#{e.message}")
    log_activity('execution_failed', level: :error, details: { error: e.message, order_id: order_id })
    notify_signal_failure([e.message])
  end

  # Writing a row fires after-commit callbacks (a broadcast, the account-sync enqueue) that can
  # raise with the row already committed. The row stays, the reporting failure is logged, and no
  # second row is ever written for the same call.
  def record_outcome
    yield
  rescue StandardError => e
    Rails.logger.error("execute_signal bot=#{id} event=outcome_record_failed error=#{e.message}")
    nil
  end

  # "It broke", once — not "it is still broken" every thirty seconds. The state is the newest
  # REGULAR transaction that is not skipped: a skip is not a recovery.
  def signal_failing?
    transactions.regular.where.not(status: :skipped).order(created_at: :desc, id: :desc).first&.failed? || false
  end

  def notify_signal_failure(errors)
    notify_about_error(errors: [exchange.humanize_error(errors.to_sentence)])
  rescue StandardError => e
    Rails.logger.error("execute_signal bot=#{id} event=notification_failed error=#{e.message}")
  end

  # A signal has no interval and no per-order amount; those columns keep their defaults, and nothing
  # reads them.
  def base_order_values(order_data = {})
    { transaction_type: order_data[:transaction_type].presence || 'REGULAR', exchange: exchange }
  end
end
