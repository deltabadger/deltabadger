# Selling off assets that a composition has dropped.
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
  def exited_holdings(data = metrics_with_current_prices)
    values = data[:asset_values] || {}
    return [] if values.empty?

    in_index = bot_index_assets.in_index.includes(:asset).to_set { |bia| bia.asset.symbol }
    # No composition on record means we do not KNOW the target — a bot whose first refresh has not
    # landed, or one whose derivation failed. Reading that as "the composition is empty" would mark
    # every holding as exited and offer to liquidate the entire portfolio.
    return [] if in_index.empty?

    tickers_by_symbol = tickers.index_by(&:base)

    values.filter_map do |symbol, asset_data|
      next if in_index.include?(symbol)

      { ticker: tickers_by_symbol[symbol], symbol: symbol }.merge(asset_data)
    end
  end

  # The tickers a liquidation would actually trade — NOT bot.tickers, which for a composition bot is every
  # quote-matching ticker in the catalogue. Market-hours checks have to ask about these: Alpaca skips
  # the stock clock only when EVERY supplied ticker is crypto, so asking with the full catalogue
  # refuses a 24/7 crypto sale any time the stock market happens to be shut.
  def liquidation_tickers(symbol: nil)
    holdings = exited_holdings
    holdings = holdings.select { |holding| holding[:symbol] == symbol } if symbol.present?
    holdings.filter_map { |holding| holding[:ticker] }.presence || tickers.to_a
  end

  # The exited holdings by NAME, with no prices involved. exited_holdings needs a live-priced hash, and the
  # Sell button must not 404 just because the five-minute cache went cold between the render and the
  # click — membership is knowable without any of that. Same two rules as exited_holdings: an empty
  # composition means we do not KNOW the target, so nothing is exited.
  def exited_symbols
    in_index = bot_index_assets.in_index.includes(:asset).to_set { |bia| bia.asset.symbol }
    return [] if in_index.empty?

    (metrics[:asset_breakdown] || {}).filter_map do |symbol, data|
      symbol if data[:amount].to_d.positive? && !in_index.include?(symbol)
    end
  end

  # Sells an exited holding at market. Runs under Bot::ActionJob's exchange semaphore (see
  # Bot::LiquidateExitedJob), which is what makes the "no placement of ours is running" reasoning in
  # Bot::LiquidationState sound.
  # One holding, named by the user from its own row. There is no sell-everything path: each of these
  # is a separate taxable disposal, and a single button over the table could not say which position
  # it was closing. The symbol arrives from the URL, so it is untrusted — a caller that names an
  # current member or something the bot does not hold is refused here as well as in the controller,
  # which keeps the job safe whatever reaches it.
  def liquidate_exited!(symbol:)
    advance_waiting_orders!
    promote_stale_liquidation_placement!

    blocked = liquidation_blocked_reason
    return Result::Failure.new(blocked) if blocked.present?

    result = refresh_composition
    # Stricter than Bot::Composition::Rebalancer#before_rebalance, which is best-effort: a stale
    # composition there rebalances toward slightly wrong weights, but here it would SELL an asset
    # that may have re-entered the composition since the page was rendered.
    return Result::Failure.new(result.errors) if result.failure?

    place_liquidation_orders!(symbol: symbol)
  end

  private

  def liquidation_blocked_reason
    return :rebalance_pending if rebalance_pending?
    return :halted if liquidation_pending?
    return :orders_waiting if transactions.liquidation.waiting.exists?

    nil
  end

  def place_liquidation_orders!(symbol:)
    # metrics(force: true), NOT metrics_with_current_prices(force: true): the latter forces only its
    # own five-minute layer and still reads the thirty-day `metrics` cache underneath, so a second
    # queued click would size against a ledger that predates the first sale.
    fresh = metrics(force: true)
    holdings = exited_holdings(metrics_with_current_prices(force: true))
               .select { |holding| holding[:symbol] == symbol }
    # Re-derived above, so this also catches an asset that re-entered the composition between the click and
    # the run — the one case where refusing is the whole point.
    return Result::Failure.new(:not_a_quitter) if holdings.empty?

    placed = 0
    holdings.each do |holding|
      outcome = liquidate_holding!(holding, fresh)
      placed += 1 if outcome == :placed
      # An unknown outcome halts the whole batch: nothing else may trade until the user has resolved
      # it, and continuing would place orders the halt is supposed to be blocking.
      break if outcome == :ambiguous
    end

    Result::Success.new(placed: placed)
  end

  def liquidate_holding!(holding, fresh)
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

    submit_liquidation!(order_data, amount_info)
  end

  def liquidation_order_data(holding, fresh)
    ticker = holding[:ticker]
    price = side_price(ticker, :sell)
    return nil if price.nil? || price <= 0

    held = fresh.dig(:asset_breakdown, holding[:symbol], :amount).to_d
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

  def submit_liquidation!(order_data, amount_info)
    # Intent BEFORE the network call. A worker that dies mid-placement must leave evidence, or the
    # next attempt sells again on top of an order that may have landed.
    start_liquidation_placement!(order_data[:ticker].base)

    result = begin
      create_order(order_data, amount_info)
    rescue Client::AmbiguousPlacementError => e
      return halt_liquidation!(order_data, "placement outcome unknown: #{e.message}")
    rescue Client::TransientNetworkError => e
      # Bot::ExchangeUser re-raises only what it proved PRE-transmission, so nothing reached the
      # venue and there is nothing to be ambiguous about.
      clear_liquidation_pending!
      return skip_liquidation({ symbol: order_data[:ticker].base }, "transient: #{e.message}")
    end

    return handle_liquidation_failure(result, order_data) if result.failure?

    order_id = result.data[:order_id]
    # Accepted but no usable id: the venue may hold a live order we can never look up again.
    return halt_liquidation!(order_data, 'placement returned no order id') if order_id.blank?

    persist_liquidation!(order_data, order_id)
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
  def handle_liquidation_failure(result, order_data)
    return halt_liquidation!(order_data, "placement failed: #{result.errors.to_sentence}") unless exchange.placement_transient_error?(result.errors)

    clear_liquidation_pending!
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

  def waiting_buy_for?(ticker)
    transactions.waiting.where(side: :buy, base: ticker.base).exists?
  end
end
