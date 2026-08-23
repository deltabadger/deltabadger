# Putting a liquidation's proceeds back into the composition, on the user's command.
#
# Selling a holding the composition dropped leaves the money in `books[:realised_cash]`, and nothing
# buys it back: the rebalance leg only swaps between members, and the DCA leg absorbs it a
# contribution at a time (`apply_regular_buy` drains realised cash before counting new money), so a
# large sale takes `ceil(proceeds / quote_amount)` intervals to re-enter the basket.
#
# This is a peer of Bot::Rebalanceable, NOT a branch of the DCA leg: a composition bot is a portfolio
# container and the schedule is only one of the ways money reaches it. So it runs while the bot is
# STOPPED, its buys never touch the DCA carry, and it has its own placement state.
#
# Buy-only by design. There is no sell leg here — the user closes each exited position themselves
# from its own row, because each is a separate taxable disposal — so nothing ever OWES a buy and
# there is no SELLING->BUYING machine to resume.
module Bot::Composition::Redeployable
  extend ActiveSupport::Concern

  PENDING_KEY = 'redeploy_pending'.freeze

  STATE_PLACING = 'placing'.freeze
  STATE_AMBIGUOUS = 'ambiguous'.freeze

  # --- the offer ------------------------------------------------------------------------------
  #
  # Derived from two MONOTONIC quantities — total liquidation proceeds and total redeploy spend,
  # both sums of `quote_amount_exec`, which only grows as fills land — minus a snapshot of the same
  # expression taken when the user last declined.
  #
  # Monotonic is the whole point. Two earlier designs stored a cash figure instead: a watermark
  # clamped down at read time (so a sale arriving between two renders was swallowed) and a
  # `created_at` cutoff (so was a pre-cutoff order that kept filling — `created_at` is when an order
  # was PLACED, and the column is whole-second precision anyway). Neither could be right without
  # someone having looked at the page at the right moment. These two never need correcting.

  def redeploy_offer(data = metrics)
    offerable = redeploy_banked - redeploy_spent - declined_offset
    # Not reachable while decline_redeploy! refuses mid-flight, so treat it as the bug state it is:
    # a stranded offset would otherwise sit above the banked total forever, silently eating every
    # future sale.
    reanchor_declined_offset! if offerable.negative?
    # realised_cash is floored at zero by construction (every drain takes a `min` against it), so
    # the clamp bounds can never cross.
    offerable.clamp(0.to_d, data[:realised_cash].to_d)
  end

  # "Forget what is on offer." Stored as the offset that makes the expression above read zero right
  # now; later sales push `redeploy_banked` past it and are offered on their own.
  #
  # REFUSED while a redeploy is working. The offset is a snapshot of `banked - spent`, so a decline
  # taken mid-batch freezes it against a `spent` that is still growing: banked 100 against spent 40
  # writes 60, the remaining 60 then fills, and the expression sits at -60 — eating the next sale.
  # Hiding the button does not prevent this; a stale tab or a double submit arrives here anyway.
  # Runs from Bot::DeclineRedeployJob, which holds the exchange semaphore — the same reason
  # Bot::ResolveLiquidationJob is a job rather than a controller line. Guarding on the in-flight
  # state alone was not enough: between the worker sizing its orders and writing its placing intent
  # there is a window where a decline sees neither a pending state nor a waiting row, succeeds, and
  # the worker then places anyway against a figure the user has just turned down. Sharing the
  # semaphore removes the window instead of narrowing it.
  def decline_redeploy!
    return Result::Failure.new(:redeploy_in_flight) if redeploy_in_flight?

    update_columns(redeploy_declined_offset: redeploy_banked - redeploy_spent)
    Result::Success.new
  end

  def redeploy_banked = executed_quote_total(transactions.liquidation)
  def redeploy_spent  = executed_quote_total(transactions.redeploy)
  def declined_offset = redeploy_declined_offset.to_d

  # The same figure the ledger counts, summed in SQL — including the fallback.
  #
  # Bot::FetchAndUpdateOrderJob explicitly allows a CLOSED order whose base fill is known while its
  # quote fill is still nil, and `confirmed_exec_amounts` values those at `price * amount`. A plain
  # SUM(quote_amount_exec) reads them as zero, so `realised_cash` would show proceeds this method
  # could not see — and the offer, capped by the smaller of the two, would sit at zero forever with
  # real money behind it.
  def executed_quote_total(scope)
    closed = Transaction.external_statuses[:closed]
    scope.submitted.sum(Arel.sql(<<~SQL.squish)).to_d
      COALESCE(quote_amount_exec,
               CASE WHEN external_status = #{closed} AND price IS NOT NULL AND amount IS NOT NULL
                    THEN price * amount ELSE 0 END)
    SQL
  end

  # --- placement state ------------------------------------------------------------------------
  # Same two states and the same reasoning as Bot::LiquidationState: `placing` means a network call
  # may be in flight right now and must not be resolvable; `ambiguous` means the outcome is unknown
  # and definitely not still running, which is the only state the halt UI accepts. The `id` is a
  # generation nonce so a stale tab cannot clear a halt raised by a later event.
  #
  # A buy needs this as much as a sell does. The tempting argument — that a retry is self-limiting
  # because an order that landed has already reduced free quote — holds only for a user with no
  # other quote on the venue. For anyone else the second click buys again with money that was never
  # the bot's.

  def redeploy_pending
    value = transient_data[PENDING_KEY]
    value.is_a?(Hash) ? value.symbolize_keys : nil
  end

  def redeploy_pending? = redeploy_pending.present?
  def redeploy_ambiguous? = redeploy_pending&.dig(:state) == STATE_AMBIGUOUS

  # Intent OR an order still working: both mean money is spoken for.
  def redeploy_in_flight?
    redeploy_pending? || transactions.redeploy.waiting.exists?
  end

  def start_redeploy_placement!
    merge_transient_data!(PENDING_KEY => { 'id' => SecureRandom.hex(6), 'state' => STATE_PLACING })
  end

  def flag_redeploy_ambiguous!
    pending = redeploy_pending
    return if pending.nil?

    merge_transient_data!(PENDING_KEY => { 'id' => pending[:id], 'state' => STATE_AMBIGUOUS })
  end

  def clear_redeploy_pending!
    merge_transient_data!(PENDING_KEY => nil)
  end

  # --- execution ------------------------------------------------------------------------------

  # Spends the offer across the composition's underweight members. Runs under Bot::ActionJob's
  # exchange semaphore (see Bot::RedeployJob), which is what makes "no placement of ours is running"
  # sound when a stale `placing` intent is promoted.
  def redeploy!
    advance_waiting_redeploys!
    promote_stale_redeploy_placement!

    # Refreshed BEFORE the market-hours check: a refresh can rotate a closed-market member into the
    # composition, and asking the old member list would let the batch through anyway. Strict, like
    # liquidation and unlike rebalancing — we are buying members, and a stale composition buys a coin
    # that has just left.
    result = refresh_composition
    return Result::Failure.new(result.errors) if result.failure?

    # Asked about the members actually being bought, and only after the refresh above — a stock
    # composition must not place into a closed market, and a refresh can rotate a closed-market
    # member in after an earlier check would have passed.
    return Result::Failure.new(:market_closed) unless exchange.market_open?(tickers: composition_tickers)

    blocked = redeploy_blocked_reason
    return Result::Failure.new(blocked) if blocked.present?

    amount = redeployable_amount
    return Result::Failure.new(:nothing_to_redeploy) unless amount.positive?

    place_redeploy_orders!(amount)
  end

  # Advances this leg's own waiting orders. Bot::FetchAndUpdateOrderJob is one-shot, and a STOPPED
  # bot never runs the tick that would otherwise sweep — so without this a redeploy order seen once
  # as open stays `waiting` forever, and waiting rows are both the double-click guard and the
  # decline's refusal, so one would wedge the feature permanently.
  #
  # update_missed_quote_amount: true, matching the liquidation sweep. The sweep covers EVERY waiting
  # row, not only this leg's, and the job already gates the carry adjustment on REGULAR internally —
  # passing false would record a REGULAR fill swept alongside without drawing down the carry, so that
  # money gets bought a second time.
  def advance_waiting_redeploys!
    watched = transactions.redeploy.waiting.pluck(:id)
    return if watched.empty?

    Bot::FetchAndUpdateOpenOrdersJob.perform_now(self, update_missed_quote_amount: true)
    halt_abandoned_redeploys!(watched)
  rescue StandardError => e
    Rails.logger.warn("redeploy order refresh failed bot=#{id}: #{e.message}")
  end

  # Everything the OTHER legs have to wait for, asked in a way that can also resolve itself — the
  # mirror of Bot::LiquidationState#liquidation_blocks_trading?.
  #
  # Without this the new leg was the only one nobody gated on: the DCA tick, a rebalance and a
  # liquidation would each place happily while a redeploy's outcome was unknown and its cash may
  # already have been spent. Every caller holds the exchange semaphore, which is what makes the
  # stale-intent promotion inside sound.
  def redeploy_blocks_trading?
    advance_waiting_redeploys!
    promote_stale_redeploy_placement!
    redeploy_in_flight?
  end

  # Called under the exchange semaphore, so `placing` here cannot be a placement still running — it
  # is a worker that died before it could rescue.
  def promote_stale_redeploy_placement!
    return false unless redeploy_pending&.dig(:state) == STATE_PLACING

    flag_redeploy_ambiguous!
    log_activity('redeploy_ambiguous', level: :error,
                                       details: { reason: 'placement intent survived its worker' })
    broadcast_redeploy_state
    true
  end

  def broadcast_redeploy_state
    broadcast_metrics_update if respond_to?(:broadcast_metrics_update)
  rescue StandardError => e
    Rails.logger.warn("redeploy broadcast failed bot=#{id}: #{e.message}")
  end

  private

  def redeploy_blocked_reason
    return :halted if redeploy_pending?
    return :orders_waiting if transactions.redeploy.waiting.exists?
    # Its proceeds are mid-flight and owed to its own buy leg.
    return :rebalance_pending if rebalance_pending?
    # A resting liquidation sell is a live claim, and its proceeds are not in yet. The self-healing
    # form, which sweeps and promotes before answering.
    return :liquidation_pending if liquidation_blocks_trading?

    nil
  end

  # metrics(force: true), NOT metrics_with_current_prices(force: true): the latter forces only its
  # own five-minute layer and still reads the thirty-day ledger underneath, so a second queued click
  # would size against a ledger that predates the first redeploy.
  #
  # Capped by the venue's own free balance: `realised_cash` is the app's belief, and nothing tells it
  # if the user withdrew the proceeds instead.
  def redeployable_amount
    [redeploy_offer(metrics(force: true)), live_free_balance(quote_asset_id)].min
  end

  def place_redeploy_orders!(amount)
    result = get_orders_data(amount, market: true)
    return result if result.failure?

    remaining = amount
    placed = 0

    result.data.each do |order_data|
      break if remaining <= 0

      outcome = place_one_redeploy!(order_data, remaining)
      # An unknown outcome halts the whole batch: nothing else may trade until the user has resolved
      # it, and continuing would place orders the halt is supposed to be blocking.
      break if outcome == :ambiguous

      remaining -= outcome[:spent] if outcome.is_a?(Hash)
      placed += 1 if outcome.is_a?(Hash)
    end

    Result::Success.new(placed: placed)
  end

  # Re-prices at PLACEMENT time rather than trusting the sizing pass. The two reads are moments
  # apart, and a venue whose market order is an emulated crossing limit reads the price a third time
  # inside the placement itself — so a rise between them would spend more than the budget while
  # `remaining` fell by only what we intended.
  def place_one_redeploy!(order_data, remaining)
    ticker = order_data[:ticker]
    price_result = exchange.market_price_for(ticker: ticker, side: :buy)
    return skip_redeploy(ticker, 'unpriced') if price_result.failure?

    price = price_result.data.to_d
    return skip_redeploy(ticker, 'unpriced') unless price.positive?

    spend = [order_data[:quote_amount].to_d, remaining].min
    return skip_redeploy(ticker, 'budget_exhausted') unless spend.positive?

    order_data = order_data.merge(price: price, quote_amount: spend, amount: spend / price,
                                  transaction_type: 'REDEPLOY')

    # Sized, then haircut if it goes out base-denominated, then checked against the venue minimum —
    # in that order. Checking first and haircutting after let an order that had just cleared the
    # minimum drop back under it, and the venue's rejection of an undersized order is not a
    # transient error, so it halted the whole batch as ambiguous over a rounding difference.
    amount_info = calculate_best_amount_info(order_data)
    if amount_info[:amount_type] == :base
      order_data = order_data.merge(amount: base_headroom_amount(order_data))
      amount_info = calculate_best_amount_info(order_data)
    end
    return skip_redeploy(ticker, 'below_minimum') if amount_info[:below_minimum_amount]

    submit_redeploy!(order_data, amount_info, spend)
  end

  # ponytail: a flat 1% haircut whenever the order goes out base-denominated, because a base amount
  # cannot express a quote cap — the venue crosses at whatever the book says when it gets there, and
  # Hyperliquid re-reads the price to build its crossing limit after we have sized. Swap for a
  # venue-reported slippage bound if one ever appears. A quote-denominated order submits the exact
  # figure and skips this entirely.
  #
  # Keyed off the RESOLVED amount_type, not the venue: under :base_or_quote and :base_and_quote,
  # calculate_best_amount_info picks per ticker, so those venues land on base for some pairs.
  BASE_HEADROOM = 0.01.to_d

  def base_headroom_amount(order_data)
    order_data[:ticker].adjusted_amount(
      amount: order_data[:amount].to_d / (1.to_d + BASE_HEADROOM), amount_type: :base
    )
  end

  def submit_redeploy!(order_data, amount_info, spend)
    # Intent BEFORE the network call. A worker that dies mid-placement must leave evidence, or the
    # next attempt buys again on top of an order that may have landed.
    start_redeploy_placement!

    result = begin
      create_order(order_data, amount_info)
    rescue Client::AmbiguousPlacementError => e
      return halt_redeploy!(order_data, "placement outcome unknown: #{e.message}")
    rescue Client::TransientNetworkError => e
      # Bot::ExchangeUser re-raises only what it proved PRE-transmission, so nothing reached the
      # venue and there is nothing to be ambiguous about.
      clear_redeploy_pending!
      return skip_redeploy(order_data[:ticker], "transient: #{e.message}")
    end

    return handle_redeploy_failure(result, order_data) if result.failure?

    order_id = result.data[:order_id]
    # Accepted but no usable id: the venue may hold a live order we can never look up again.
    return halt_redeploy!(order_data, 'placement returned no order id') if order_id.blank?

    persist_redeploy!(order_data, order_id)
    { spent: spend }
  end

  # The insert and the intent clear commit TOGETHER, so "a row exists but the intent survived" is an
  # impossible state — which is what lets a surviving intent be read as a dead worker.
  def persist_redeploy!(order_data, order_id)
    transaction = nil
    ActiveRecord::Base.transaction do
      transaction = persist_accepted_order!(order_data, order_id)
      clear_redeploy_pending!
    end
    Bot::FetchAndUpdateOrderJob.perform_later(transaction, update_missed_quote_amount: false)
    log_activity('redeploy_placed', details: order_log_details(order_data))
  end

  # A Result::Failure is NOT by itself proof that nothing was placed — only placement_transient_error?,
  # which matches strings guaranteeing a PRE-TRADE rejection, is trustworthy enough to unwind.
  def handle_redeploy_failure(result, order_data)
    return halt_redeploy!(order_data, "placement failed: #{result.errors.to_sentence}") unless exchange.placement_transient_error?(result.errors)

    clear_redeploy_pending!
    create_failed_order!(order_data.merge(error_messages: result.errors, transaction_type: 'REDEPLOY'))
    :failed
  end

  # Terminal halt, never a retry — a placement without an order id is reconcilable by neither
  # get_orders nor Bot::StaleOrderResolver, so there is no automatic recovery to claim.
  def halt_redeploy!(order_data, reason)
    flag_redeploy_ambiguous!
    log_activity('redeploy_ambiguous', level: :error,
                                       details: order_log_details(order_data).merge(reason: reason))
    broadcast_redeploy_state
    :ambiguous
  end

  def skip_redeploy(ticker, reason)
    log_activity('redeploy_skipped', level: :info,
                                     details: { base: ticker&.base_asset&.symbol, reason: reason })
    :skipped
  end

  # Bot::StaleOrderResolver marks an order the venue stopped reporting as `abandoned` after 14 days.
  # That is not proof it never executed, and once the row leaves `waiting` nothing stops a fresh
  # redeploy placing on top of a fill we never recorded — so it becomes the same halt.
  def halt_abandoned_redeploys!(watched_ids)
    return if watched_ids.empty? || redeploy_pending?

    abandoned = transactions.redeploy.where(id: watched_ids, external_status: :abandoned).first
    return if abandoned.nil?

    merge_transient_data!(PENDING_KEY => { 'id' => SecureRandom.hex(6), 'state' => STATE_AMBIGUOUS })
    log_activity('redeploy_ambiguous', level: :error,
                                       details: { base: abandoned.base,
                                                  reason: 'exchange stopped reporting the order' })
    broadcast_redeploy_state
  end

  def reanchor_declined_offset!
    Rails.logger.warn(
      "redeploy offset stranded above banked total bot=#{id} " \
      "offset=#{declined_offset} banked=#{redeploy_banked} spent=#{redeploy_spent}"
    )
    update_columns(redeploy_declined_offset: redeploy_banked - redeploy_spent)
  end
end
