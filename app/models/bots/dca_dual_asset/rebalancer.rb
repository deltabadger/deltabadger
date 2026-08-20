# The rebalance leg's execution: sell the overweight asset, buy the underweight one with the
# proceeds. Kept apart from OrderSetter (the DCA leg) because the two legs share only their target
# weights — the DCA leg spends new money and never sells, this one spends nothing and must sell.
#
# Placement is a two-step flow across a network, so it is written as a resumable state machine
# rather than a straight line. See Bot::Rebalanceable for the state itself and why a pending
# rebalance blocks a new one.
module Bots::DcaDualAsset::Rebalancer
  extend ActiveSupport::Concern

  include Bot::OrderSetter

  # Entry point for Bot::RebalanceJob. Resume always wins: while state exists the only legal move is
  # to finish it, whatever the current drift says.
  def rebalance!
    return resume_rebalance! if rebalance_pending?
    return Result::Success.new(skipped: :not_due) unless rebalance_due?

    start_rebalance!
  end

  private

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
    # and hand the next poll a drift reading that makes it sell the OTHER asset.
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

  # Sell the overweight asset down to its target. Sized to target rather than to the band edge, so
  # one correction ends the excursion instead of leaving it hovering on the threshold.
  def rebalance_sell_order_data
    data = metrics_with_current_prices
    return nil if data[:prices_stale]

    value0 = data[:total_base0_amount_value_in_quote].to_d
    value1 = data[:total_base1_amount_value_in_quote].to_d
    total = value0 + value1
    return nil unless total.positive?

    target0 = total * allocation0.to_d
    overweight0 = value0 > target0
    ticker = overweight0 ? ticker0 : ticker1
    excess_in_quote = overweight0 ? value0 - target0 : value1 - (total - target0)
    return nil unless excess_in_quote.positive?

    price = side_price(ticker, :sell)
    return nil if price.nil? || price <= 0

    # Never sell more than is actually on the exchange: holdings the user moved to cold storage are
    # part of the portfolio for allocation purposes but cannot be traded.
    amount = [excess_in_quote / price, live_free_balance(ticker.base_asset_id)].min
    return nil unless amount.positive?

    order_data(ticker:, price:, amount:, quote_amount: amount * price, side: :sell)
  end

  def rebalance_buy_order_data(quote_amount:)
    return nil if quote_amount.nil? || quote_amount <= 0

    data = metrics_with_current_prices
    value0 = data[:total_base0_amount_value_in_quote].to_d
    value1 = data[:total_base1_amount_value_in_quote].to_d
    total = value0 + value1
    # Buy back into whichever side is now light. After a sell that is the other asset by
    # construction, but reading it fresh keeps a resumed buy correct if the market moved meanwhile.
    ticker = value0 < (total * allocation0.to_d) ? ticker0 : ticker1

    price = side_price(ticker, :buy)
    return nil if price.nil? || price <= 0

    # Capped by the live quote balance: fees make the actual proceeds a little less than the gross
    # exec amount, and without the cap the buy would quietly dip into the DCA leg's cash.
    # ponytail: the residual fee-sized imprecision is left uncorrected — exact net proceeds would
    # need per-venue fee data, and it self-corrects at the next rebalance.
    spend = [quote_amount.to_d, live_free_balance(quote_asset_id)].min
    return nil unless spend.positive?

    order_data(ticker:, price:, amount: spend / price, quote_amount: spend, side: :buy)
  end

  def order_data(ticker:, price:, amount:, quote_amount:, side:)
    {
      ticker: ticker,
      price: price,
      amount: amount,
      quote_amount: quote_amount,
      side: side,
      order_type: limit_ordered? ? :limit_order : :market_order,
      # Not a contribution. Keeps rebalance fills out of the DCA carry, the spend cap and
      # Bot#last_transaction.
      transaction_type: 'REBALANCE'
    }
  end

  # Market orders cross the spread on the side they trade; limit orders sit behind it. The DCA path
  # only ever needed the buy side of this, so both directions live here.
  def side_price(ticker, side)
    result = if limit_ordered?
               ticker.get_last_price
             elsif side == :sell
               ticker.get_bid_price
             else
               ticker.get_ask_price
             end
    return nil if result.failure?

    return result.data unless limit_ordered?

    distance = limit_order_pcnt_distance_decimal
    multiplier = side == :sell ? (1.to_d + distance) : (1.to_d - distance)
    ticker.adjusted_price(price: result.data * multiplier)
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
