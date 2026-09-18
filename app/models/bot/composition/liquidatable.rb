# Selling holdings at the user's request: constituents the composition has dropped, or any current
# member — direct indexing is exactly the freedom to close a position on its own. Never a side
# effect of rebalancing (see the quitter reasoning below), always positions the user named, always a
# taxable disposal they picked the moment for.
#
# Deliberately NOT part of rebalancing. Rebalancing tracks the composition and steers members toward
# their weights; an exited holding has no weight to steer toward, and folding it in produced two bad
# behaviours at once: with target 0 it was liquidated automatically the moment any OTHER asset
# breached the band (churning a member that merely hovers at the boundary in and out),
# while an asset that left at 0.1% of the portfolio never tripped the band on its own and was
# therefore never sold at all.
#
# So it is manual. Closing the position is a taxable disposal, and the user picks the moment.
module Bot::Composition::Liquidatable
  extend ActiveSupport::Concern

  # Holdings the bot still owns that its composition no longer wants, in the shape the table renders and
  # the sell trades. One source for both, so the button can never disagree with what is on screen.
  #
  # Built from priced holdings, so a DELISTED asset does not appear: it has no ticker, no price,
  # and no way to be sold. That matches the main table, which has never shown it either.
  #
  # Every row names its holding's key AND its asset: a key's text can change (a rename, a second asset
  # with the same symbol), so whatever acts on a row checks the asset it was shown.
  def exited_holdings(data = metrics_with_current_prices)
    in_index = bot_index_assets.in_index.pluck(:asset_id).to_set
    # No composition on record means we do not KNOW the target — a bot whose first refresh has not
    # landed, or one whose derivation failed. Reading that as "the composition is empty" would mark
    # every holding as exited and offer to liquidate the entire portfolio.
    return [] if in_index.empty?

    # The dust rule belongs HERE and only here — on assets the composition no longer wants. Size
    # alone is not what makes a holding worth showing; size against the desired allocation is. A
    # member holding 0.0001 stays on the page because the bot is going to buy more of it, while a
    # quitter holding 0.0001 is a remainder no sale can clear and nothing plans to add to.
    sellable_holdings(data).reject { |holding| in_index.include?(holding[:asset_id]) }
  end

  # Judged on the venue's base floor, which needs no price: an amount under it cannot be submitted at
  # all. The quote floor is checked later, at placement, where a price is actually in hand.
  def sellable?(ticker, amount)
    return false if ticker.nil?

    amount.to_d >= ticker.minimum_base_size.to_d
  end

  # Every holding the bot could sell right now, members and quitters alike, in the row shape. Only a
  # holding whose asset is known: one recorded solely by a symbol string is never sold — a sale is recorded
  # under its asset and could never reduce it.
  def sellable_holdings(data = metrics_with_current_prices)
    key_assets = data[:key_assets] || {}
    (data[:asset_values] || {}).filter_map do |key, asset_data|
      asset_id = key_assets[key]
      ticker = asset_id && ticker_for_asset(asset_id)
      next unless sellable?(ticker, asset_data[:amount])

      { ticker:, symbol: key, asset_id: }.merge(asset_data)
    end
  end

  # The sellable holdings by KEY, with their asset, no prices involved — what the controller and the API
  # validate a request against, so a cold price cache cannot turn a live Sell button into a 404.
  def held_assets
    payload = metrics
    (payload[:asset_breakdown] || {}).each_with_object({}) do |(key, data), acc|
      asset_id = (payload[:key_assets] || {})[key]
      acc[key] = asset_id if asset_id && sellable?(ticker_for_asset(asset_id), data[:amount])
    end
  end

  def held_symbols = held_assets.keys

  # The tickers a liquidation would actually trade — NOT bot.tickers, which for a composition bot is every
  # quote-matching ticker in the catalogue. Market-hours checks have to ask about these: Alpaca skips
  # the stock clock only when EVERY supplied ticker is crypto, so asking with the full catalogue
  # refuses a 24/7 crypto sale any time the stock market happens to be shut.
  def liquidation_tickers(holdings: [])
    # Named holdings resolve straight off the ticker table, NOT through priced holdings: a name whose
    # price read failed silently drops out of sellable_holdings, and a [crypto, stock] batch that
    # loses its stock here is judged all-crypto, skips the stock clock, and then sells the stock
    # anyway once place_liquidation_orders! forces a fresh price. Membership needs no price.
    if holdings.present?
      held = held_assets
      return holdings.filter_map { |key, asset_id| ticker_for_asset(asset_id || held[key]) }.presence || tickers.to_a
    end

    sellable_holdings.filter_map { |holding| holding[:ticker] }.presence || tickers.to_a
  end

  # The exited holdings by NAME, with no prices involved. exited_holdings needs a live-priced hash, and the
  # Sell button must not 404 just because the five-minute cache went cold between the render and the
  # click — membership is knowable without any of that. Same two rules as exited_holdings: an empty
  # composition means we do not KNOW the target, so nothing is exited.
  def exited_symbols
    in_index = bot_index_assets.in_index.pluck(:asset_id).to_set
    return [] if in_index.empty?

    # Same dust rule as exited_holdings (held_assets is sellable holdings only), or the controller would
    # accept a key the table does not show — a Sell that 404s from the page and one refused by the job.
    held_assets.reject { |_key, asset_id| in_index.include?(asset_id) }.keys
  end

  # Sells at market. Runs under Bot::ActionJob's exchange semaphore (see Bot::LiquidateExitedJob),
  # which is what makes the "no placement of ours is running" reasoning in Bot::LiquidationState
  # sound — and `deadline` is when that semaphore's lease runs out, so the run can stop before the
  # reasoning stops holding.
  #
  # The positions the user named — one row's Sell, or the band's Sell all carrying exactly the
  # symbols that table rendered. Each is still a separate taxable disposal: its own order, its own
  # transaction, its own wash-sale verdict and lock, its own activity row, nothing netted. The old
  # objection to a bulk button was that it could not say which position it was closing; this one
  # routes through a confirmation that names every one of them, and the intent slot still holds the
  # single symbol being placed, so a halt says which sale is in doubt and nothing after it is tried.
  #
  # The holdings arrive from the URL, so they are untrusted — `[key, asset_id]` pairs, resolved when the
  # request was made. A key the bot does not hold, or one that now names another asset, is refused here as
  # well as in the controller, which keeps the job safe whatever reaches it.
  def liquidate!(holdings:, deadline: nil)
    advance_waiting_orders!(sweep_if: transactions.sell.waiting)
    promote_stale_liquidation_placement!

    blocked = liquidation_blocked_reason
    return Result::Failure.new(blocked) if blocked.present?

    # Best-effort, exactly like Bot::Composition::Rebalancer#before_rebalance. The refusal used to be
    # strict so that a quitter which had re-entered the composition could not be sold; a current
    # member may now be sold on purpose, so a stale composition is no longer a reason to decline. The
    # refresh still runs, so the row this sale touches says whether the name is in the index.
    result = refresh_composition
    Rails.logger.warn("liquidate bot=#{id} composition refresh failed: #{result.errors.to_sentence}") if result.failure?

    place_liquidation_orders!(holdings: holdings, deadline: deadline)
  end

  private

  def liquidation_blocked_reason
    return :rebalance_pending if rebalance_pending?
    return :halted if liquidation_pending?
    # Any sell of ours, not just a liquidation: a scheduled (DCA-out) sell resting on the book would be
    # sold a second time underneath it. liquidate! refreshes these first, so a stale `open` does not
    # block for good.
    return :orders_waiting if transactions.sell.waiting.exists?
    # An order the venue stopped reporting is not proof it never executed, and being abandoned takes
    # it out of `waiting` — so without this it would stop blocking anything the moment we gave up on
    # it, and a fresh sale could place on top of a fill we never recorded.
    return :orders_unresolved if unresolved_liquidation_orders.exists?
    # A redeploy is buying the members with cash a previous sale realized. Selling underneath it —
    # or worse, selling while its outcome is unknown — trades against money that may already be
    # committed. try: this concern is shared with types that have no redeploy leg.
    return :redeploy_pending if try(:redeploy_blocks_trading?)

    nil
  end

  def place_liquidation_orders!(holdings:, deadline: nil)
    # metrics(force: true), NOT metrics_with_current_prices(force: true): the latter forces only its
    # own five-minute layer and still reads the thirty-day `metrics` cache underneath, so a second
    # queued click would size against a ledger that predates the first sale.
    fresh = metrics(force: true)
    by_key = sellable_holdings(metrics_with_current_prices(force: true)).index_by { |holding| holding[:symbol] }
    # .uniq is load-bearing, not tidiness: a repeated name would place the same holding twice, both
    # sized off this one snapshot. The order is the confirmation's, so the feed reads as listed.
    named = Array(holdings).map { |key, asset_id| [key, asset_id] }.uniq(&:first)
    # A key whose holding is now another asset than the one the request named is not the position the
    # user approved. nil expected: a request queued before holdings carried their asset.
    holdings = named.filter_map do |key, expected|
      holding = by_key[key]
      holding if holding && (expected.nil? || holding[:asset_id] == expected.to_i)
    end
    return Result::Failure.new(:not_held) if holdings.empty?

    # A named position with no sellable holding right now — no price in the refreshed read, no
    # ticker, or under the venue floor — is dropped here. For a single sale that WAS the whole sale
    # and the refusal above said so; in a batch the others still sell, so each dropped one has to
    # say why on its own. Otherwise the user asks for three, gets two, and nothing anywhere
    # explains the third.
    (named.map(&:first) - holdings.map { |holding| holding[:symbol] }).each do |key|
      skip_liquidation({ symbol: key }, 'not_held')
    end

    deadline ||= liquidation_batch_deadline
    placed = 0
    holdings.each_with_index do |holding, index|
      # The exchange semaphore is this run's clock: Solid Queue takes it at DISPATCH, never renews
      # it, and queue delay spends it too. Once it lapses a second sale can run beside us, and
      # between holdings there is no intent and no waiting row for it to stand down on — so both
      # runs could size the same position. Stop starting holdings before that gets likely; what is
      # left stays on the table and the user clicks again.
      if Time.current > deadline
        cut_liquidation_batch_short(holdings.drop(index))
        break
      end

      outcome = liquidate_holding!(holding, fresh, deadline)
      placed += 1 if outcome == :placed
      # An unknown outcome halts the whole batch: nothing else may trade until the user has resolved
      # it, and continuing would place orders the halt is supposed to be blocking.
      break if outcome == :ambiguous
    end

    Result::Success.new(placed: placed)
  end

  # The fallback when no lease can be read — a direct call, or a run with no semaphore row behind it.
  # Bot::LiquidateExitedJob passes the real expiry, which is the only thing that accounts for queue
  # delay. Do NOT raise the job's `duration:` to buy room instead: that also delays the sweep which
  # recovers a worker that died holding the lock.
  def liquidation_batch_deadline = Time.current + 1.minute

  def cut_liquidation_batch_short(untried)
    log_activity('liquidation_batch_cut_short', level: :info,
                                                details: { bases: untried.map { |holding| holding[:symbol] }.join(', ') })
  end

  def liquidate_holding!(holding, fresh, deadline = nil)
    ticker = holding[:ticker]
    return skip_liquidation(holding, 'unavailable') unless ticker&.available? && ticker.trading_enabled?
    # With FeeCutter on, the DCA leg can have a resting limit buy for an asset that has since exited.
    # Selling underneath it just re-acquires the position when it fills, so leave this one and say
    # why — the rest of the batch is unaffected.
    return skip_liquidation(holding, 'open_order') if waiting_buy_for?(ticker)

    order_data = liquidation_order_data(holding, fresh)
    return skip_liquidation(holding, 'unpriced') if order_data.nil?

    amount_info = calculate_best_amount_info(order_data)
    return skip_liquidation(holding, 'below_minimum') if amount_info[:below_minimum_amount]

    # Asked AGAIN, because liquidation_order_data just made two network reads and each can take tens
    # of seconds — so the window that was open at the top of the loop may be gone. This is the last
    # moment a stop is free: no intent recorded, nothing sent. Placing past the lease is what lets
    # another sale run beside this one and size the same position.
    return skip_liquidation(holding, 'exclusion_lapsed') if deadline && Time.current > deadline

    submit_liquidation!(order_data, amount_info, loss: sell_at_loss?(order_data))
  end

  def liquidation_order_data(holding, fresh)
    ticker = holding[:ticker]
    price = side_price(ticker, :sell)
    return nil if price.nil? || price <= 0

    held = fresh.dig(:asset_breakdown, key_for(ticker.base_asset_id, fresh), :amount).to_d
    # Never sell more than is actually on the exchange: holdings the user moved to cold storage are
    # part of the portfolio for accounting but cannot be traded.
    amount = [held, live_free_balance(ticker.base_asset_id)].min
    return nil unless amount.positive?

    {
      ticker: ticker,
      price: price,
      amount: amount,
      quote_amount: amount * price,
      side: :sell,
      order_type: :market_order,
      transaction_type: 'LIQUIDATION'
    }
  end

  def submit_liquidation!(order_data, amount_info, loss:)
    asset_id = order_data[:ticker].base_asset_id
    # Intent BEFORE the network call, and the wash-sale lock WITH it, in one transaction: a worker
    # that dies mid-placement must leave both — an order that may have landed, and a name that must
    # not be bought back. The previous deadline is kept so a provable non-placement can put it back:
    # a locked remainder may be sold again, and that failing must not unlock the earlier sale.
    previous_lock = nil
    ActiveRecord::Base.transaction do
      start_liquidation_placement!(order_data[:ticker].base)
      previous_lock = lock_buying!(asset_id) if loss # a claim Hash, or nil
    end

    result = begin
      create_order(order_data, amount_info)
    rescue Client::AmbiguousPlacementError => e
      # The sale may have happened, so the lock stays.
      return halt_liquidation!(order_data, "placement outcome unknown: #{e.message}")
    rescue Client::TransientNetworkError => e
      # Bot::ExchangeUser re-raises only what it proved PRE-transmission, so nothing reached the
      # venue and there is nothing to be ambiguous about — and nothing was sold, so nothing to guard.
      clear_liquidation_pending!
      restore_buy_lock!(asset_id, previous_lock) if loss
      return skip_liquidation({ symbol: order_data[:ticker].base }, "transient: #{e.message}")
    end

    return handle_liquidation_failure(result, order_data, loss:, previous_lock:) if result.failure?

    order_id = result.data[:order_id]
    # Accepted but no usable id: the venue may hold a live order we can never look up again.
    return halt_liquidation!(order_data, 'placement returned no order id') if order_id.blank?

    outcome = persist_liquidation!(order_data, order_id)
    log_wash_sale_lock(asset_id) if loss
    outcome
  end

  # The insert and the intent clear commit TOGETHER. That is what makes "a row exists but intent
  # survived" an impossible state, which in turn means an intent never has to be matched back to a
  # row — just as well, since created_at is second-precision and would happily match the PREVIOUS
  # liquidation of the same symbol.
  def persist_liquidation!(order_data, order_id)
    transaction = nil
    ActiveRecord::Base.transaction do
      transaction = persist_accepted_order!(order_data, order_id)
      clear_liquidation_pending!
    end
    Bot::FetchAndUpdateOrderJob.perform_later(transaction, update_missed_quote_amount: false)
    log_activity('liquidation_placed', details: order_log_details(order_data))
    :placed
  end

  # A Result::Failure is NOT by itself proof that nothing was placed — only
  # placement_transient_error?, which matches strings that guarantee a PRE-TRADE rejection, is
  # trustworthy enough to unwind. Everything else halts.
  def handle_liquidation_failure(result, order_data, loss:, previous_lock:)
    return halt_liquidation!(order_data, "placement failed: #{result.errors.to_sentence}") unless exchange.placement_transient_error?(result.errors)

    clear_liquidation_pending!
    restore_buy_lock!(order_data[:ticker].base_asset_id, previous_lock) if loss
    create_failed_order!(order_data.merge(error_messages: result.errors, transaction_type: 'LIQUIDATION'))
    :failed
  end

  # Terminal halt, never a retry. Bot::ActionJob documents that a placement without an order id is
  # reconcilable by neither get_orders nor Bot::StaleOrderResolver — get_orders needs an id we never
  # stored, and this codebase has no exchange-wide recent-order discovery — so there is no automatic
  # recovery to claim. The user checks the venue and clears it from the widget.
  def halt_liquidation!(order_data, reason)
    flag_liquidation_ambiguous!
    log_activity('liquidation_ambiguous', level: :error,
                                          details: order_log_details(order_data).merge(reason: reason))
    broadcast_liquidation_state
    :ambiguous
  end

  def skip_liquidation(holding, reason)
    log_activity('liquidation_skipped', level: :info,
                                        details: { base: holding[:symbol], reason: reason })
    :skipped
  end

  # Every bot on the account, not just this one: the lock is the taxpayer's, so a resting buy
  # anywhere on the account is what would undo this sale — by asset, or under any name for it.
  def waiting_buy_for?(ticker) = account_waiting_buys(ticker).exists?
end
