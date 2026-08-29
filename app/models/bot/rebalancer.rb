# The rebalance leg's execution, shared by every multi-asset bot type: sell the overweight asset,
# buy the underweight one with the proceeds. Kept apart from the OrderSetter concerns (the DCA leg)
# because the two legs share only their target weights — the DCA leg spends new money and never
# sells, this one spends nothing and must sell.
#
# Placement is a two-step flow across a network, so it is written as a resumable state machine
# rather than a straight line. See Bot::Rebalanceable for the state itself and why a pending
# rebalance blocks a new one.
#
# EVERYTHING here is shape-agnostic. A bot type joins in by implementing one method:
#
#   rebalance_targets => [{ ticker:, value: <in quote>, target: <weight 0..1> }, ...]
#             or nil when the portfolio cannot be valued right now (stale prices, no holdings)
#
# From that list the drift, the asset to sell and the asset to buy all follow, so a two-asset bot
# and a fifty-asset index bot run the identical machine.
module Bot::Rebalancer
  extend ActiveSupport::Concern

  include Bot::OrderSetter

  # Entry point for Bot::RebalanceJob. Resume always wins: while state exists the only legal move is
  # to finish it, whatever the current drift says.
  def rebalance!
    # The ambiguous halt is terminal until the user resolves it, and nothing may move underneath it
    # either: before_rebalance refreshes a basket's composition, which on a bot that has just lost
    # a member would drop it and renormalise the rest while the halt below did nothing.
    return Result::Success.new(skipped: :ambiguous) if rebalance_ambiguous?

    before_rebalance
    return resume_rebalance! if rebalance_pending?
    return Result::Success.new(skipped: :not_due) unless rebalance_due?

    start_rebalance!
  end

  private

  # Types whose asset set can change under them refresh it here. A fixed pair has nothing to do.
  def before_rebalance; end

  def start_rebalance!
    order_data = rebalance_sell_order_data
    # Below the venue minimum: the portfolio has drifted but not by enough to trade. Deliberately
    # silent — no Transaction row and no activity entry, because this repeats every poll for as long
    # as the drift persists. The widget's live drift readout is where the user sees it.
    return Result::Success.new(skipped: :below_minimum) if order_data.nil?

    # Intent BEFORE the network call: a worker that dies mid-placement must leave evidence that
    # blocks a new sell, or the next poll sells again on top of an order that may have landed.
    set_rebalance_pending!(phase: Bot::Rebalanceable::PHASE_SELLING)
    place_rebalance_order(order_data, phase: Bot::Rebalanceable::PHASE_SELLING)
  end

  def resume_rebalance!
    return Result::Success.new(skipped: :ambiguous) if rebalance_ambiguous?

    case rebalance_pending[:phase]
    when Bot::Rebalanceable::PHASE_SELLING then resume_selling!
    when Bot::Rebalanceable::PHASE_BUYING then resume_buying!
    else
      # Intent persisted, no transaction, no phase we recognise — the placement outcome is unknown.
      halt_ambiguous!('unrecognised pending phase')
    end
  end

  def resume_selling!
    transaction = pending_transaction(:sell_transaction_id)
    # A crash between persisting intent and persisting the order leaves no transaction to inspect,
    # and no read path can find an order whose id we never stored. Never guess — halt.
    return halt_ambiguous!('sell intent with no persisted order') if transaction.nil?

    refresh_pending_order!(transaction)
    return Result::Success.new(skipped: :waiting_for_sell) unless terminal?(transaction)
    return halt_ambiguous!('sell abandoned') if transaction.abandoned?

    proceeds = realized_proceeds(transaction)
    # nil, not zero: Bot::FetchAndUpdateOrderJob explicitly allows a closed sell whose amount_exec is
    # known while quote_amount_exec is still nil. Reading that as "sold nothing" would clear the state
    # and strand real cash, so wait for a figure we can actually spend.
    return Result::Success.new(skipped: :proceeds_unknown) if proceeds.nil?

    # A cancelled sell can still carry a partial fill. Clearing it would strand that cash invisibly
    # and hand the next poll a drift reading that makes it sell a DIFFERENT asset.
    return clear_and_succeed!(:nothing_sold) unless proceeds.positive?

    set_rebalance_pending!(
      phase: Bot::Rebalanceable::PHASE_BUYING,
      sell_transaction_id: transaction.id,
      remaining_quote_amount: proceeds
    )
    place_pending_buy!
  end

  def resume_buying!
    transaction = pending_transaction(:buy_transaction_id)
    if transaction.nil? && rebalance_pending[:buy_transaction_id].blank?
      # Attempted, but no order id landed in the state: the worker may have died after the venue
      # accepted it. Replaying would spend the proceeds twice. Only a handoff that never reached the
      # network is safe to place.
      return halt_ambiguous!('buy attempted with no persisted order') if rebalance_pending[:buy_attempted]

      return place_pending_buy!
    end
    return halt_ambiguous!('buy intent with no persisted order') if transaction.nil?

    refresh_pending_order!(transaction)
    # Not cleared at placement: an unfilled buy is cash committed but not yet converted, and the next
    # poll would read that as fresh drift and start a second rebalance.
    return Result::Success.new(skipped: :waiting_for_buy) unless terminal?(transaction)
    return halt_ambiguous!('buy abandoned') if transaction.abandoned?

    filled = realized_proceeds(transaction)
    return Result::Success.new(skipped: :fill_unknown) if filled.nil?

    remaining = rebalance_remaining_quote_amount.to_d - filled
    return clear_and_succeed!(:completed) unless remaining.positive?

    # A partially filled then cancelled buy still owes the remainder. Retry only that.
    set_rebalance_pending!(
      phase: Bot::Rebalanceable::PHASE_BUYING,
      sell_transaction_id: rebalance_pending[:sell_transaction_id],
      remaining_quote_amount: remaining
    )
    place_pending_buy!
  end

  # Closed/cancelled orders carry the quote figure most of the time, but not always — fall back to
  # the base amount at the fill price, and return nil rather than guessing zero when neither exists.
  def realized_proceeds(transaction)
    return transaction.quote_amount_exec.to_d if transaction.quote_amount_exec.present?
    return 0.to_d if transaction.amount_exec.present? && transaction.amount_exec.zero?
    return nil if transaction.amount_exec.blank? || transaction.price.blank?

    transaction.amount_exec.to_d * transaction.price.to_d
  end

  def place_pending_buy!
    owed = rebalance_remaining_quote_amount.to_d
    return clear_and_succeed!(:nothing_owed) unless owed.positive?

    order_data = rebalance_buy_order_data(quote_amount: owed)
    # nil here means the order could not be BUILT — a failed price read, a quote balance that has not
    # settled yet. Those are transient, and treating them as dust would clear an owed buy and let a
    # later poll sell again. Only a spend proven below the venue minimum is dust, and that decision
    # belongs to place_rebalance_order, which can actually measure it.
    return Result::Success.new(skipped: :buy_unavailable) if order_data.nil?

    # Mark the attempt before the network call, so a worker death after acceptance cannot be replayed.
    mark_buy_attempted!
    place_rebalance_order(order_data, phase: Bot::Rebalanceable::PHASE_BUYING)
  end

  def mark_buy_attempted!
    pending = rebalance_pending
    set_rebalance_pending!(
      phase: Bot::Rebalanceable::PHASE_BUYING,
      sell_transaction_id: pending[:sell_transaction_id],
      remaining_quote_amount: rebalance_remaining_quote_amount,
      buy_attempted: true
    )
  end

  # --- placement ----------------------------------------------------------------------------

  def place_rebalance_order(order_data, phase:)
    amount_info = calculate_best_amount_info(order_data)
    # Nothing was placed, so the intent persisted a moment ago must not survive — a stale pending
    # row would block every future rebalance, silently and forever.
    return abandon_below_minimum!(phase, order_data) if amount_info[:below_minimum_amount]

    result = begin
      create_order(order_data, amount_info)
    rescue Client::AmbiguousPlacementError => e
      # Bot::ExchangeUser converts an unknown-outcome network error into this. The order may be live.
      return halt_ambiguous!("#{order_data[:side]} placement outcome unknown: #{e.message}")
    rescue Client::TransientNetworkError => e
      # ExchangeUser re-raises only what it proved PRE-transmission — nothing reached the venue, so
      # the sell owes nothing and the buy is still safely retryable next cycle.
      clear_rebalance_pending! if phase == Bot::Rebalanceable::PHASE_SELLING
      return Result::Failure.new(e.message)
    end
    return handle_placement_failure(result, phase:, order_data:) if result.failure?

    order_id = result.data[:order_id]
    # Accepted but no usable id: the venue may hold a live order we can never look up again.
    return halt_ambiguous!('placement returned no order id') if order_id.blank?

    clear_rebalance_below_minimum!
    transaction = persist_accepted_order!(order_data, order_id)
    carry_pending_forward(phase:, transaction:)
    Bot::FetchAndUpdateOrderJob.perform_later(transaction, update_missed_quote_amount: false)
    log_activity("rebalance_#{order_data[:side]}_placed", details: order_log_details(order_data))
    Result::Success.new(transaction_id: transaction.id)
  end

  # A Result::Failure is NOT by itself proof that nothing was placed. Exchange#transient_error? is
  # deliberately broader than this site can accept — a placement network timeout is indistinguishable
  # from a successful book hit, and placement has no idempotency key. Only
  # placement_transient_error?, which matches strings that guarantee a PRE-TRADE rejection, is
  # trustworthy enough to unwind. Everything else halts.
  def handle_placement_failure(result, phase:, order_data:)
    unless exchange.placement_transient_error?(result.errors)
      return halt_ambiguous!("#{order_data[:side]} placement failed: #{result.errors.to_sentence}")
    end

    create_failed_order!(order_data.merge(error_messages: result.errors, transaction_type: 'REBALANCE'))

    if phase == Bot::Rebalanceable::PHASE_SELLING
      # Provably never reached the matching engine, so nothing was sold and nothing is owed.
      clear_rebalance_pending!
    end
    # A buy that provably never placed still holds the user's cash — keep the state and the
    # remaining amount so the next cycle retries exactly that.
    result
  end

  # Below the venue floor. On the sell leg that just means the drift is real but too small to trade
  # — silent, because it repeats every poll. On the buy leg it is the user's own realized cash left
  # unconvertible, which is worth saying once.
  def abandon_below_minimum!(phase, order_data)
    if phase == Bot::Rebalanceable::PHASE_BUYING
      log_activity('rebalance_dust', level: :info,
                                     details: order_log_details(order_data))
    else
      # Silent, but not invisible: the widget reads this to explain why a breached band is not
      # producing trades.
      flag_rebalance_below_minimum!
    end
    clear_rebalance_pending!
    Result::Success.new(skipped: :below_minimum)
  end

  def carry_pending_forward(phase:, transaction:)
    if phase == Bot::Rebalanceable::PHASE_SELLING
      set_rebalance_pending!(phase: Bot::Rebalanceable::PHASE_SELLING, sell_transaction_id: transaction.id)
    else
      set_rebalance_pending!(
        phase: Bot::Rebalanceable::PHASE_BUYING,
        sell_transaction_id: rebalance_pending[:sell_transaction_id],
        buy_transaction_id: transaction.id,
        remaining_quote_amount: rebalance_remaining_quote_amount,
        buy_attempted: true
      )
    end
  end

  # Terminal halt, never a retry state. Bot::ActionJob documents that a placement without an order id
  # creates no Transaction and is reconcilable by neither get_orders nor Bot::StaleOrderResolver, so
  # there is no automatic recovery to claim: the user resolves it from the widget after checking the
  # exchange. Stranded cash beats a double sell.
  def halt_ambiguous!(reason)
    set_rebalance_pending!(
      phase: Bot::Rebalanceable::PHASE_AMBIGUOUS,
      sell_transaction_id: rebalance_pending&.dig(:sell_transaction_id),
      buy_transaction_id: rebalance_pending&.dig(:buy_transaction_id),
      # Kept so a manual resolution can hand the owed buy back rather than losing the ledger.
      remaining_quote_amount: rebalance_remaining_quote_amount,
      buy_attempted: rebalance_pending&.dig(:buy_attempted)
    )
    log_activity('rebalance_ambiguous', level: :error, details: { reason: reason })
    Result::Success.new(skipped: :ambiguous)
  end

  def clear_and_succeed!(reason)
    clear_rebalance_pending!
    Result::Success.new(skipped: reason)
  end

  def pending_transaction(key)
    id = rebalance_pending&.dig(key)
    id.present? ? transactions.find_by(id: id) : nil
  end

  # Bot::FetchAndUpdateOrderJob is one-shot and never reschedules itself, and a stopped bot has no
  # DCA tick running the open-order sweep — so without this nothing would ever advance the order and
  # the rebalance would wait forever.
  def refresh_pending_order!(transaction)
    Bot::FetchAndUpdateOrderJob.perform_now(transaction, update_missed_quote_amount: false)
    transaction.reload
  rescue StandardError => e
    Rails.logger.warn("rebalance refresh failed bot=#{id} transaction=#{transaction.id} error=#{e.message}")
  end

  def terminal?(transaction)
    transaction.closed? || transaction.cancelled? || transaction.abandoned?
  end

  # --- order data ---------------------------------------------------------------------------

  # Sell the MOST overweight asset down to its target, rather than only to the band edge.
  #
  # With two assets that single correction ends the excursion outright, because the excess it frees
  # is exactly the other side's shortfall. With more assets it does not: one poll fixes one pair,
  # and a portfolio far from its weights converges over several polls. Taking the largest excess
  # first is what makes that convergence as quick as one-pair-at-a-time allows.
  # ponytail: one sell + one buy per evaluation. Aggregating across the top-K overweights would
  # converge in a single pass, but it means several sell transactions under one pending state; the
  # shape to change is the state machine's single sell_transaction_id.
  def rebalance_sell_order_data
    entry, total = extreme_rebalance_entry(:max)
    return nil if entry.nil?

    excess_in_quote = entry[:value] - (total * entry[:target])
    return nil unless excess_in_quote.positive?

    ticker = entry[:ticker]
    price = side_price(ticker, :sell)
    return nil if price.nil? || price <= 0

    # Never sell more than is actually on the exchange: holdings the user moved to cold storage are
    # part of the portfolio for allocation purposes but cannot be traded.
    amount = [excess_in_quote / price, live_free_balance(ticker.base_asset_id)].min
    return nil unless amount.positive?

    rebalance_order_data(ticker:, price:, amount:, quote_amount: amount * price, side: :sell)
  end

  # Buy back into the MOST underweight asset. Read fresh rather than remembered, so a resumed buy
  # stays correct if the market moved while the sell was resting.
  def rebalance_buy_order_data(quote_amount:)
    return nil if quote_amount.nil? || quote_amount <= 0

    entry, total = extreme_rebalance_entry(:min)
    return nil if entry.nil?

    # The cash-inclusive total is what the portfolio will be worth once this money is deployed, so
    # the shortfall is measured against the same denominator the next drift reading will use.
    prospective_total = total + quote_amount.to_d
    shortfall = (entry[:target].to_d * prospective_total) - entry[:value].to_d
    # Nothing is under target — the most underweight asset is not actually underweight. Buying here
    # would put the proceeds straight back into something already over its weight, most likely the
    # asset just sold. Hold the cash and let a later poll place it.
    return nil unless shortfall.positive?

    ticker = entry[:ticker]
    price = side_price(ticker, :buy)
    return nil if price.nil? || price <= 0

    # Capped THREE ways, and the middle one is why an index bot does not churn: spending the whole
    # proceeds into one asset only lands exactly on target when there are two assets, because there
    # excess(overweight) == shortfall(underweight) identically. With three or more they are
    # unrelated, so an uncapped buy overshoots and the next poll sells back what this one just
    # bought — a round trip costing two fees and a taxable disposal for no change in allocation.
    # Whatever is left over rides remaining_quote_amount and resume_buying! places it into the
    # next-most-underweight asset. For two assets shortfall == the owed amount, so this cap is a
    # no-op and the pair bot's behaviour is unchanged.
    #
    # The live quote balance is the third cap: fees make the actual proceeds a little less than the
    # gross exec amount, and without it the buy would quietly dip into the DCA leg's cash.
    # ponytail: the residual fee-sized imprecision is left uncorrected — exact net proceeds would
    # need per-venue fee data, and it self-corrects at the next rebalance.
    spend = [quote_amount.to_d, shortfall, live_free_balance(quote_asset_id)].min
    return nil unless spend.positive?

    rebalance_order_data(ticker:, price:, amount: spend / price, quote_amount: spend, side: :buy)
  end

  # The entry furthest from its target, in quote terms, plus the portfolio total it was measured
  # against. :max is the most overweight, :min the most underweight.
  def extreme_rebalance_entry(direction)
    entries = rebalance_targets
    return [nil, 0] if entries.blank?

    total = entries.sum { |entry| entry[:value].to_d }
    return [nil, 0] unless total.positive?

    # Availability is checked HERE rather than in Bot::RebalanceJob because that guard reads
    # tickers_for_start, which the index bot does not implement (it would answer [] and pass
    # vacuously). Checking the asset actually about to trade is both type-agnostic and stricter: a
    # delisted or halted asset drops out of the candidates while the rest of the portfolio still
    # rebalances around it.
    tradeable = entries.select do |entry|
      entry[:ticker].present? && entry[:ticker].available? && entry[:ticker].trading_enabled?
    end
    return [nil, total] if tradeable.empty?

    deviation = ->(entry) { entry[:value].to_d - (total * entry[:target].to_d) }
    entry = direction == :max ? tradeable.max_by(&deviation) : tradeable.min_by(&deviation)
    [entry, total]
  end

  def rebalance_order_data(ticker:, price:, amount:, quote_amount:, side:)
    {
      ticker: ticker,
      price: price,
      amount: amount,
      quote_amount: quote_amount,
      side: side,
      # ALWAYS a market order, never FeeCutter's limit. FeeCutter is a DCA setting and its premise —
      # "waiting is free" — is false here. A rebalance order's size is derived from a SNAPSHOT of the
      # allocation, so a resting order executes against a picture that is hours stale. Worse, it is
      # adversely selected: a sell parked above the market fills only if the overweight asset kept
      # rising, which is exactly when the amount we computed was too small. Both legs systematically
      # under-correct, and when the price moves the other way the order never fills at all — which
      # wedges the rule, because a pending rebalance blocks every future one.
      order_type: :market_order,
      # Not a contribution. Keeps rebalance fills out of the DCA carry, the spend cap and
      # Bot#last_transaction.
      transaction_type: 'REBALANCE'
    }
  end

  # The price a market order will realistically execute at, asked of the venue rather than assumed:
  # on venues with no native market order type this is also the price actually submitted, so sizing
  # and the venue-minimum check have to use exactly this number or the order is built against one
  # price and sent at another.
  def side_price(ticker, side)
    result = exchange.market_price_for(ticker: ticker, side: side)
    return nil if result.failure?

    result.data
  end

  def live_free_balance(asset_id)
    result = get_balance(asset_id: asset_id)
    if result.failure?
      raise Client::RateLimitedError, result.errors.to_sentence if exchange.throttled_error?(result.errors)
      raise Client::TransientNetworkError, result.errors.to_sentence if exchange.transient_error?(result.errors)

      raise "Failed to read balance for bot #{id}: #{result.errors.to_sentence}"
    end

    result.data[:free].to_d
  end
end
