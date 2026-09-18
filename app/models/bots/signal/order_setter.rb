# What a signal bot does when it is told to trade: one market order, recorded like any other bot's —
# the shape of Bots::DcaSingleAsset::OrderSetter with the schedule taken out.
#
# Two callers tell it, and they differ only in who sizes the order:
#   - a webhook rule (execute_signal): the rule says how much, read against the venue's balances;
#   - an authenticated API or MCP call naming the bot (execute_api_order): the caller says how much.
# Everything after sizing is one path, so the two can never disagree about what an order was.
#
# There is no next interval to carry a lost order to, so every outcome leaves a row: a submitted,
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

  # What one attempt came to — the same thing the feed shows, handed back because one of the two
  # callers is still on the line: an API call answers with it, a webhook's job ignores it.
  #   :submitted  the venue holds the order; `transaction` is nil only if its row could not be written
  #   :skipped    below the venue's minimum; a skipped row
  #   :failed     nothing reached the venue; a failed row
  #   :ambiguous  the request may have gone out; no row, never retried
  #   :locked     a wash-sale lock refused the buy; nothing recorded
  Outcome = Data.define(:status, :transaction, :order_id, :errors) do
    def self.of(status, transaction: nil, order_id: nil, errors: [])
      new(status:, transaction:, order_id:, errors: Array(errors))
    end
  end

  def execute_signal(signal)
    side = signal.direction.to_sym
    # Before signal_order_data: that one reads the venue and can record a failure the user is emailed
    # about, all for a signal that is about to be skipped anyway. The asset is known from the bot's
    # own ticker, so nothing it produces is needed here.
    return Outcome.of(:locked) if buy_locked?(side)

    order_data = signal_order_data(signal)
    return Outcome.of(:failed) if order_data.nil?

    amount_info = checked_amount_info(side, order_data)
    return amount_info if amount_info.is_a?(Outcome)

    place_and_track(side, order_data, amount_info)
  end

  # The caller's number is the order: no balance is read and nothing is capped to the wallet — a
  # caller that asks for more than it holds is told so by the venue, in a failed row.
  def execute_api_order(side:, amount:, amount_type:)
    # Everything before the placement call. A raise in here reached no venue, so it is a failed row;
    # the webhook path gets the same from Bot::SignalJob's own rescue. Placement is deliberately
    # OUTSIDE this rescue: past that line an exception no longer proves nothing was sent.
    begin
      return Outcome.of(:locked) if buy_locked?(side)

      order_data = api_order_data(side, amount, amount_type)
      amount_info = checked_amount_info(side, order_data)
    rescue StandardError => e
      return record_signal_failure(side, [e.message])
    end
    return amount_info if amount_info.is_a?(Outcome)

    amount_info = amount_info.merge(amount:, amount_type:) if venue_takes?(side, amount_type)
    place_and_track(side, order_data, amount_info)
  end

  # A failure nothing reached the venue for: one row, and one email decision, whatever raised it.
  def record_signal_failure(side, errors, order_data: nil)
    order_data ||= { ticker: ticker, side: side, order_type: :market_order }
    Rails.logger.error(
      "execute_signal bot=#{id} event=order_failed #{order_log_fields(order_data)} errors=#{errors.to_sentence}"
    )
    notify = !signal_failing? # read BEFORE the row that would answer "yes"
    transaction = record_outcome { create_failed_order!(order_data.merge(error_messages: errors)) }
    notify_signal_failure(errors) if notify
    Outcome.of(:failed, transaction:, errors:)
  end

  private

  def buy_locked?(side)
    return false unless side == :buy && user.locked_asset_ids.include?(ticker.base_asset_id)

    Rails.logger.info("execute_signal bot=#{id} event=order_wash_sale_locked base=#{ticker.base}")
    true
  end

  # Whether the caller's denomination can reach the venue as sent. Where it can, it does, exactly
  # as on an order without a bot: "spend 100" must not become "buy 0.002" at a price that has moved
  # by the time it lands. Where the venue takes only the other one — whole shares are base only,
  # notional orders quote only, and every bot in the app sizes a sell in base (one venue raises on a
  # quote-sized sell before any request leaves) — calculate_best_amount_info has already converted
  # it at the current price, the same conversion every scheduled bot on that venue gets.
  def venue_takes?(side, amount_type)
    return amount_type == :base if side == :sell

    case exchange.minimum_amount_logic(side: side, order_type: :market_order)
    when :base_or_quote, :base_and_quote then true
    when :quote then amount_type == :quote
    else amount_type == :base
    end
  end

  # The last checks before placement, for both callers: the venue's minimum, and the wash-sale
  # lock re-read. Returns the amount to submit — or the Outcome that ends the attempt.
  def checked_amount_info(side, order_data)
    amount_info = calculate_best_amount_info(order_data)
    if amount_info[:below_minimum_amount]
      Rails.logger.info("execute_signal bot=#{id} event=order_skipped #{order_log_fields(order_data)}")
      log_activity('order_skipped', level: :warning, details: order_log_details(order_data))
      return Outcome.of(:skipped, transaction: record_outcome { create_skipped_order!(order_data) })
    end

    # Re-read the lock immediately before placing, exactly as the composition leg does: this order
    # was sized before another exchange's semaphore let a sale through.
    return Outcome.of(:locked) if buy_locked?(side)

    amount_info
  end

  # Nothing in here raises: placement answers for its own failures, and so does tracking.
  def place_and_track(side, order_data, amount_info)
    placed = place_signal_order(side, order_data, amount_info)
    return placed unless placed.status == :submitted

    placed.with(transaction: track_signal_order(order_data, placed.order_id))
  end

  # Sizing a rule. Every read here is idempotent and retried in place; what still fails becomes a
  # failed row. Sells are sized in base at the bid, never beyond what is free — the wallet, not the
  # bot's own history, is the ceiling, exactly as for a selling DCA bot.
  def signal_order_data(signal)
    side = signal.direction.to_sym
    price = signal_price(side)
    amount, quote_amount = signal_size(signal, side, price)
    { ticker: ticker, price: price, amount: amount, quote_amount: quote_amount, side: side, order_type: :market_order }
  rescue StandardError => e
    record_signal_failure(signal.direction.to_sym, [e.message])
    nil
  end

  # Sizing an API order: the amount as given, the other side of it at the price a market order
  # would execute at. Raises on an unreadable price; execute_api_order turns that into a failed row.
  def api_order_data(side, amount, amount_type)
    price = signal_price(side)
    base, quote = amount_type == :base ? [amount, amount * price] : [amount / price, amount]
    { ticker: ticker, price: price, amount: base, quote_amount: quote, side: side, order_type: :market_order }
  end

  # The price a market order would execute at, from the venue: on the venues that emulate a
  # market order by crossing the spread (Hyperliquid, Gemini) that is the touch plus the cross,
  # and sizing off the raw touch would spend more than was asked.
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
  def place_signal_order(side, order_data, amount_info)
    Rails.logger.info("execute_signal bot=#{id} event=order_creating #{order_log_fields(order_data)}")
    result = create_order(order_data, amount_info)
    result = create_order(order_data, amount_info) if result.failure? && exchange.placement_transient_error?(result.errors)

    if result.success?
      order_id = result.data[:order_id]
      return Outcome.of(:submitted, order_id:) if exchange.acknowledged_order_id?(order_id)

      return ambiguous_placement(order_data, 'the venue accepted the order but returned no order id')
    end

    return ambiguous_placement(order_data, result.errors.to_sentence) if exchange.ambiguous_placement_error?(result)

    record_signal_failure(side, result.errors, order_data: order_data)
  rescue StandardError => e
    ambiguous_placement(order_data, e.message)
  end

  def ambiguous_placement(order_data, message)
    Rails.logger.warn(
      "execute_signal bot=#{id} event=placement_ambiguous #{order_log_fields(order_data)} error=#{message}"
    )
    log_activity('placement_ambiguous', level: :warning, details: order_log_details(order_data).merge(error: message))
    Outcome.of(:ambiguous, errors: [message])
  end

  # From here the venue holds the order. Nothing that goes wrong on our side may be written down as
  # a failed order; the submitted row and its exchange id are what lets the fill be confirmed. A row
  # that could not be written, or a confirmation that could not be queued, is said out loud with
  # the exchange order id, so it can be reconciled by hand. Returns the row, or nil if there is none.
  def track_signal_order(order_data, order_id)
    Rails.logger.info("execute_signal bot=#{id} event=order_accepted order_id=#{order_id} #{order_log_fields(order_data)}")
    transaction = persist_accepted_order!(order_data, order_id)
    raise 'the confirmation job was not enqueued' unless Bot::FetchAndUpdateOrderJob.perform_later(transaction)

    transaction
  rescue StandardError => e
    Rails.logger.error("execute_signal bot=#{id} event=accepted_order_untracked order_id=#{order_id} error=#{e.message}")
    log_activity('execution_failed', level: :error, details: { error: e.message, order_id: order_id })
    notify_signal_failure([e.message])
    # An after-commit callback can raise with the row already written, before the assignment above
    # completed — so the row is looked for rather than assumed absent.
    transaction || accepted_row(order_id)
  end

  def accepted_row(order_id)
    transactions.find_by(external_id: order_id)
  rescue StandardError
    nil
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
  # reads them. Everything else a row carries is the shared part, taken whole.
  def base_order_values(order_data = {})
    order_identity_values(order_data)
  end
end
