module Bot::Composition::Measurable
  extend ActiveSupport::Concern

  # The fill arithmetic here backs every composition bot (index, basket) so they can never
  # disagree about what a rebalance does to holdings, cost basis and realized cash.
  include Bot::RebalanceAccounting

  # Bounded so 100-asset bots don't stampede the exchange/proxy; tail fetches after
  # CandleSeriesCache are one small request each, so 6 workers keep latency flat.
  CANDLE_FETCH_THREADS = 6

  def metrics(force: false)
    Rails.cache.fetch(metrics_cache_key, expires_in: 30.days, force: force) do
      data = initialize_metrics_data
      transactions_array = transactions.submitted.order(created_at: :asc).pluck(
        :id,
        :created_at,
        :price,
        :amount_exec,
        :quote_amount_exec,
        :amount,
        :base,
        :side,
        :external_status,
        :transaction_type
      )
      return data if transactions_array.empty?

      totals = initialize_totals_data
      ledger = Hash.new { |hash, key| hash[key] = { amount: 0, invested: 0 } }
      # The tax-shaped view of the same fills (Bot::TaxLots), kept beside the performance ledger
      # because a rebalance moves performance basis between assets and tax basis never travels.
      lots = Hash.new { |hash, key| hash[key] = [] }
      tax_pnl = {}
      loss_lot = {}
      books = new_rebalance_books
      asset_prices = {} # Track last known price for each asset
      # Corporate actions are events in this walk like any fill. A pending queue rather than a
      # merge, because they are rare and this loop runs over every order the bot ever placed.
      pending_splits = split_events

      transactions_array.each do |id, created_at, price, amount_exec, quote_amount_exec, amount, base, side, external_status, transaction_type|
        # Before the order, not after: the restatement is a property of the position the order then
        # acts on, so a split sharing an order's timestamp is applied first.
        pending_splits = apply_due_splits(pending_splits, created_at, ledger, asset_prices, data, books, lots)

        raw_amount_exec = amount_exec
        raw_quote_exec = quote_amount_exec
        amount_exec, quote_amount_exec =
          confirmed_exec_amounts(external_status, price, amount, amount_exec, quote_amount_exec)

        # The tax view of the fill (Bot::TaxLots), BEFORE the performance skip below: a fill the
        # performance walk cannot use (units executed, proceeds not reported — an open or cancelled
        # partial) still moved units for tax purposes.
        #   Sell: the verdict is judged on the RAW proceeds, per transaction, so the fill path can
        #   start or extend a wash-sale lock from what actually happened; with none reported it is
        #   unknown (nil), which the guard reads as a loss. The units are gone either way, so the
        #   lots are consumed either way — the next sale must be judged against what remains.
        #   Buy: a lot opens at what was actually paid — the reported proceeds when the venue gave
        #   them, else the order price times the units the venue said were executed. Never
        #   confirmed_exec_amounts' fallback, which prices the REQUESTED amount: a closed buy that
        #   asked for 2, got 1 and reported no proceeds would otherwise open one unit costing two.
        if amount_exec.to_d.positive?
          if side == 'sell'
            if raw_quote_exec.present? && raw_quote_exec.to_d.positive?
              tax_pnl[id] = raw_quote_exec.to_d - Bot::TaxLots.cost_of(lots[base], amount_exec)
              loss_lot[id] = Bot::TaxLots.loss_in?(lots[base], amount_exec, raw_quote_exec)
            else
              loss_lot[id] = nil
            end
            Bot::TaxLots.consume(lots[base], amount_exec)
          else
            # Alpaca reports a ZERO, not a blank, when it has no average fill price yet, so only a
            # positive figure counts as reported cost. Failing that, the order price times the
            # executed units; failing THAT, the lot's cost is unknown (nil) — never zero, which
            # would turn any later sale of it into a "gain".
            reported = raw_quote_exec.to_d.positive? ? raw_quote_exec.to_d : nil
            estimated = price.to_d.positive? ? price.to_d * (raw_amount_exec || amount).to_d : nil
            lots[base] << { amount: amount_exec.to_d, cost: reported || estimated }
          end
        end

        # A sell the venue executed but did not price — judged on the RAW proceeds column, before
        # confirmed_exec_amounts' fallback invents `price × requested amount` for a closed order.
        # The units are gone, so the performance ledger must lose them too — the next Sell sizes on
        # this quantity, and so do the buy legs' offsets — or a second Sell could submit shares the
        # bot no longer holds. Proceeds unknown: assumed equal to the basis those units carried, so
        # the sale is P/L-neutral until the venue reports them (a later poll that does recomputes
        # this walk from the row). Applied whatever the order's price says, and WITHOUT touching the
        # asset's last mark: an unpriced sale has no price to mark anything with.
        if side == 'sell' && amount_exec.to_d.positive? && !raw_quote_exec.to_d.positive?
          apply_fill(ledger, books, key: base, side:, transaction_type:,
                                    amount_exec:, quote_amount_exec: basis_share(ledger, base, amount_exec))
          # A chart point, at the marks that already stand: without one the last snapshot still holds
          # the units this sale removed, and chart_marked_at_market keeps pricing them at market for
          # the whole segment after it. Both sides move together, so the curve gains a vertex, not a
          # step — the same reasoning as a split's point in apply_due_splits.
          data[:chart][:labels] << created_at
          data[:chart][:series][0] << portfolio_value(ledger_value(ledger, asset_prices), books)
          data[:chart][:series][1] << invested_total(books)
          data[:chart][:extra_series] << ledger.transform_values { |entry| entry[:amount] }
          data[:chart][:invested_series] << ledger.transform_values { |entry| entry[:invested] }
          (data[:chart][:cash_series] ||= []) << uninvested_cash(books)
          next
        end

        next if price.blank? || quote_amount_exec.blank? || amount_exec.blank?
        next if quote_amount_exec.zero? || amount_exec.zero?

        branch = apply_fill(ledger, books, key: base, side:, transaction_type:,
                                           amount_exec:, quote_amount_exec:)
        asset_prices[base] = price

        # Chart data
        data[:chart][:labels] << created_at
        data[:chart][:series][0] << portfolio_value(ledger_value(ledger, asset_prices), books)
        data[:chart][:series][1] << invested_total(books)

        # Store snapshot of per-asset amounts for candle interpolation
        data[:chart][:extra_series] << ledger.transform_values { |entry| entry[:amount] }
        # And of what each asset cost, which is the zero line of its own curve when the user
        # hovers its row. NOT derivable from the totals: a rebalance moves basis between assets.
        data[:chart][:invested_series] << ledger.transform_values { |entry| entry[:invested] }
        (data[:chart][:cash_series] ||= []) << uninvested_cash(books)

        # Metrics data — average entry price is over REGULAR buys only; a swap is not an entry.
        if branch == :regular_buy
          totals[:prices] << price
          totals[:amounts] << amount_exec
        end
      end

      # The ordinary case: a split lands and the bot has not traded since.
      apply_due_splits(pending_splits, nil, ledger, asset_prices, data, books, lots)

      data[:total_quote_amount_invested] = invested_total(books)
      # Cash realized by a sell whose buy has not landed yet — or by a liquidation the bot has not
      # re-spent — is still the user's money.
      data[:rebalance_cash] = uninvested_cash(books)
      # Split out from rebalance_cash on purpose. That one also carries a half-finished swap's flight
      # cash, which is owed to its own buy leg; only this half is money a redeploy may spend.
      data[:realised_cash] = books[:realised_cash]
      data[:realised_pnl] = realised_pnl(books)
      data[:total_amount_value_in_quote] = portfolio_value(ledger_value(ledger, asset_prices), books)
      data[:pnl] = calculate_pnl(data[:total_quote_amount_invested], data[:total_amount_value_in_quote])
      # Public shape kept as-is (:quote_invested, not the ledger's :invested) — the order setter, the
      # live-price pass and the chart all read it. Plain hash: a default proc will not cache.
      data[:asset_breakdown] = ledger.each_with_object({}) do |(symbol, entry), acc|
        acc[symbol] = { amount: entry[:amount], quote_invested: entry[:invested],
                        tax_basis: Bot::TaxLots.basis(lots[symbol]),
                        tax_units: lots[symbol].sum { |lot| lot[:amount] },
                        tax_cost_unknown: Bot::TaxLots.unknown_cost?(lots[symbol]) }
      end
      # Plain hash of plain arrays: the lots a sale is judged against (Bot::WashSaleGuard).
      data[:asset_lots] = lots.transform_values { |list| list.map(&:dup) }
      data[:tax_pnl_by_transaction] = tax_pnl
      data[:loss_lot_by_transaction] = loss_lot
      data[:num_assets] = ledger.count { |_symbol, entry| entry[:amount].positive? }

      data
    end
  end

  def metrics_with_current_prices(force: false)
    Rails.cache.fetch(metrics_with_current_prices_cache_key,
                      expires_in: Utilities::Time.seconds_to_end_of_five_minute_cut,
                      force: force) do
      metrics_data = metrics.deep_dup
      return metrics_data if metrics_data[:chart][:labels].empty?

      # A restatement the market has not priced yet: the venue's latest trade is still the old
      # basis, and this walk's holdings are already the new one.
      return stale(metrics_data) if restated_prices_untrusted?(metrics_data)

      result = exchange.get_tickers_prices(symbols: tickers.map(&:ticker))
      return stale(metrics_data) if result.failure?

      ticker_prices = result.data
      total_value = 0

      # Raw per-symbol marks for the chart's price grid (see Bot::ChartSeries). Only symbols
      # that actually priced land here: a symbol missing from this hash has no upper endpoint,
      # so the chart leaves those points on their fill marks instead of marking a portfolio
      # that is silently missing an asset.
      live_prices = {}

      # Calculate current value for each asset
      asset_values = {}
      metrics_data[:asset_breakdown].each do |symbol, asset_data|
        # A fully liquidated holding keeps its ledger row (the chart reads holdings by position and the
        # series must stay parallel) but has nothing left to show. Without this a composition bot that
        # rebalances accumulates a zero row per asset it has ever rotated out of.
        next unless asset_data[:amount].positive?

        ticker = tickers.find { |t| t.base == symbol }
        next unless ticker.present?

        price = ticker_prices[ticker.ticker]
        next unless price.present?

        live_prices[symbol] = price
        value = asset_data[:amount] * price
        total_value += value
        avg_price = asset_data[:amount].positive? ? asset_data[:quote_invested] / asset_data[:amount] : 0
        pnl_pct = asset_data[:quote_invested].positive? ? (value - asset_data[:quote_invested]) / asset_data[:quote_invested] : 0
        # The colour is judged on the TAX quantity, not the performance quantity — a sale the
        # performance walk skipped (no proceeds) still took its units out of the lots — and never on
        # a position whose lots include one of unknown cost.
        tax_units = asset_data[:tax_units].to_d
        asset_values[symbol] = {
          amount: asset_data[:amount],
          quote_invested: asset_data[:quote_invested],
          current_value: value,
          current_price: price,
          avg_price: avg_price,
          pnl_percentage: pnl_pct,
          tax_basis: asset_data[:tax_basis],
          harvestable: tax_units.positive? && !asset_data[:tax_cost_unknown] &&
                       tax_units * price < asset_data[:tax_basis].to_d
        }
      end

      total_value += metrics_data[:rebalance_cash].to_d
      metrics_data[:total_amount_value_in_quote] = total_value
      metrics_data[:pnl] = calculate_pnl(metrics_data[:total_quote_amount_invested], total_value)
      metrics_data[:asset_values] = asset_values
      metrics_data[:chart][:series][0] << total_value
      metrics_data[:chart][:series][1] << metrics_data[:total_quote_amount_invested]
      metrics_data[:chart][:labels] << Time.current
      # extra_series stays parallel with labels — the chart reads holdings by position.
      metrics_data[:chart][:extra_series] << metrics_data[:asset_breakdown].transform_values { |data| data[:amount] }
      (metrics_data[:chart][:invested_series] ||= []) <<
        metrics_data[:asset_breakdown].transform_values { |data| data[:quote_invested] }
      (metrics_data[:chart][:cash_series] ||= []) << metrics_data[:rebalance_cash].to_d
      metrics_data[:live_prices] = live_prices

      metrics_data
    end
  end

  def metrics_with_current_prices_and_candles(force: false)
    Rails.cache.fetch(metrics_with_current_prices_and_candles_cache_key,
                      expires_in: Utilities::Time.seconds_to_end_of_five_minute_cut,
                      force: force) do
      metrics_data = metrics_with_current_prices(force: force).deep_dup
      return metrics_data if metrics_data[:chart][:labels].empty?

      grids = chart_price_grids(metrics_data)
      return metrics_data if grids.blank?

      # Only symbols the bot can price at all take part. A bot can rotate its composition, so it can
      # hold assets it no longer tracks (the user liquidates them by hand — see
      # Bot::Composition::Liquidatable) — a DELISTED one has no ticker,
      # and `metrics_with_current_prices` already leaves them out of the live value. Demanding
      # candle coverage for them would leave almost every point uncovered (28 held symbols, 20
      # tickered, on a real bot) while the headline priced only the 20. A symbol that HAS a
      # ticker but no candles still blocks: there the chart would disagree with a headline that
      # prices it live.
      priceable = tickers.map(&:base)
      metrics_data[:chart] = chart_marked_at_market(
        metrics_data[:chart], grids,
        display_grids: chart_display_grids(metrics_data, grids),
        holdings: ->(i) { (metrics_data[:chart][:extra_series][i] || {}).slice(*priceable) },
        basis: ->(i) { ((metrics_data[:chart][:invested_series] || [])[i] || {}).slice(*priceable) },
        # Cash a rebalance sell realized but its buy has not spent yet — and, when a remainder ends
        # as dust, permanently. Marking those points at zero would draw a loss that never happened.
        cash: ->(i) { (metrics_data[:chart][:cash_series] || [])[i] || 0 }
      )

      metrics_data
    end
  end

  def broadcast_metrics_update
    broadcast_metrics_panel
    broadcast_chart
  end

  # Split out because the chart is the expensive half — it waits on a candle series that a
  # composition change cannot alter. Anything that only moves a holding between the two tables
  # broadcasts this one on its own and lands immediately.
  def broadcast_metrics_panel
    broadcast_replace_to(
      ["user_#{user_id}", :bot_updates],
      target: 'metrics',
      partial: metrics_partial,
      locals: { bot: self, metrics: metrics_with_current_prices, loading: false,
                exited_title_key: exited_title_key }
    )
  end

  def broadcast_chart
    broadcast_replace_to(
      ["user_#{user_id}", :bot_updates],
      target: 'chart',
      partial: 'bots/chart',
      locals: { bot: self, metrics: metrics_with_current_prices_and_candles, loading: false, current_user: user }
    )
  end

  def metrics_with_current_prices_from_cache
    Rails.cache.read(metrics_with_current_prices_cache_key)
  end

  def metrics_with_current_prices_and_candles_from_cache
    Rails.cache.read(metrics_with_current_prices_and_candles_cache_key)
  end

  private

  # Holdings valued at the last price each asset traded at. Used for the chart's running value and
  # for the fallback headline when the live read fails.
  def ledger_value(ledger, asset_prices)
    ledger.sum { |symbol, entry| entry[:amount] * (asset_prices[symbol] || 0) }
  end

  # The cost basis the ledger carries for `amount` of `base` — the same proportion release_basis
  # takes, read without releasing anything.
  def basis_share(ledger, base, amount)
    return 0.to_d unless ledger.key?(base) && ledger[base][:amount].to_d.positive?

    ledger[base][:invested].to_d * [amount.to_d / ledger[base][:amount].to_d, 1].min
  end

  # _v3: realised P/L and the contributed/ledger split. _v4: a per-symbol cost-basis snapshot per
  # transaction, so the chart can draw one holding on its own. _v5: realised_cash split out of
  # rebalance_cash, which the redeploy offer reads — without the bump an existing bot serves a hash
  # with no such key for up to 30 days and the prompt never appears. _v6: holdings are restated
  # through corporate actions, so every count, value and chart point in here can differ.
  # _v7: per-asset FIFO tax lots and the harvestable flag.
  # The cached shape lives up to 30 days, so this has to move with it or every existing bot serves
  # the old numbers after a deploy.
  # A method, not a literal: the tests that seed this cache were reading the string off the source.
  def metrics_cache_key
    "bot_#{id}_metrics_v7_#{restatement_generation}"
  end

  def metrics_with_current_prices_cache_key
    "bot_#{id}_metrics_with_current_prices_v7_#{restatement_generation}"
  end

  def metrics_with_current_prices_and_candles_cache_key
    "bot_#{id}_metrics_with_current_prices_and_candles_v7_#{restatement_generation}"
  end

  def optimal_candles_timeframe_for_duration(duration)
    # We want to show a chart of ~300 points when possible
    if duration < (1 * 300).minutes
      1.minute
    elsif duration < (5 * 300).minutes
      5.minutes
    elsif duration < (15 * 300).minutes
      15.minutes
    elsif duration < (30 * 300).minutes
      30.minutes
    elsif duration < (1 * 300).hours
      1.hour
    else
      1.day
    end
  end

  # Folds in every restatement due at or before `until_time` (all of them when nil), and returns
  # what is left. A split multiplies the position and moves no money, so `invested` — and with it
  # the cost basis, the realised P/L and the rebalance books — is untouched.
  #
  # The last-known price is divided by the same factor. This walk values the portfolio at the price
  # each asset last traded at, and that price is pre-split; without restating it the value jumps by
  # the factor and stays there until the asset next trades, which is exactly the fallback headline
  # a bot shows when the live read fails.
  #
  # A chart point is emitted, as a fill would. `chart_marked_at_market` interpolates between the
  # snapshots stored here, so with no snapshot at the split every candle point between it and the
  # next order would price a pre-split count. Because both sides move together the point's value
  # equals the one before it — the curve gains a vertex, not a step.
  def apply_due_splits(pending, until_time, ledger, asset_prices, data, books, lots)
    # The overwhelming majority of bots have no corporate actions at all, and this runs once per
    # fill over histories that reach into the millions of them.
    return pending if pending.empty?

    due, rest = pending.partition { |at, _symbol, _factor| until_time.nil? || at <= until_time }
    due.each do |at, symbol, factor|
      # Tax lots first, ahead of the performance guards below: a lot opened by a fill the
      # performance walk skipped still holds units the split multiplies.
      Bot::TaxLots.split!(lots[symbol], factor) if lots.key?(symbol)
      # `key?`, not `[]`: the ledger's default proc would CREATE a zero entry for a symbol the bot
      # never actually held, and every one of those becomes a row in `asset_breakdown`.
      next unless ledger.key?(symbol)

      entry = ledger[symbol]
      next if entry[:amount].to_d.zero?

      entry[:amount] = entry[:amount].to_d * factor
      asset_prices[symbol] /= factor if asset_prices[symbol].present?
      # Only a restatement that MOVED something puts the live prices out of trust.
      data[:restated_at] = [data[:restated_at], at].compact.max

      data[:chart][:labels] << at
      data[:chart][:series][0] << portfolio_value(ledger_value(ledger, asset_prices), books)
      data[:chart][:series][1] << invested_total(books)
      data[:chart][:extra_series] << ledger.transform_values { |values| values[:amount] }
      data[:chart][:invested_series] << ledger.transform_values { |values| values[:invested] }
      (data[:chart][:cash_series] ||= []) << uninvested_cash(books)
    end
    rest
  end

  # The price grid every held symbol is marked on: its candle opens plus its live price.
  # A symbol whose candles fail to fetch simply gets no grid, and Bot::ChartSeries then leaves
  # every point where that symbol is held on its fill mark — one bad ticker must not blank the
  # chart, and it must not silently price a held asset at zero either.
  def chart_price_grids(metrics_data)
    return nil if tickers.empty?

    symbols = metrics_data[:asset_breakdown].keys
    return nil if symbols.empty?

    since, timeframe = chart_candle_window(metrics_data[:chart])
    grids = chart_candle_grids(symbols, metrics_data, since: since, timeframe: timeframe)

    # One member the candles do not cover would otherwise blank the interpolation for every member.
    labels = metrics_data[:chart][:labels]
    chart_split_pinned_grids(
      chart_backfilled_grids(grids, symbols: symbols, from: labels.first, to: labels.last)
    )
  end

  # The overlay's own grids: each restating symbol's history as its venue reads it TODAY, one
  # basis end to end. No pins and no fill backfill — both exist to carry a VALUATION across a
  # seam these grids do not have, and a fill price is as-traded, which on this basis it is not.
  #
  # Merged over the valuation grids per symbol, never wholesale: a basket can hold stocks beside
  # crypto, and one ticker whose restated fetch failed must not take the other nineteen back with
  # it. A symbol left behind keeps the raw line this chart has always drawn.
  def chart_display_grids(metrics_data, grids)
    restating = restating_symbols & metrics_data[:asset_breakdown].keys
    return grids if restating.empty?

    since, timeframe = chart_candle_window(metrics_data[:chart])
    grids.merge(chart_candle_grids(restating, metrics_data, since: since, timeframe: timeframe,
                                                            restated: true))
  end

  # Which members have a history their venue rewrites. One query for the categories rather than
  # one per member: the question reaches `ticker.base_asset`, and a chart drawn from warm candle
  # caches otherwise loads no asset row at all.
  def restating_symbols
    list = tickers
    list = list.preload(:base_asset) if list.is_a?(ActiveRecord::Relation)
    list.select(&:restated_candles?).map(&:base)
  end

  # Bounded parallel batches. A raise in one worker becomes a per-symbol failure (logged);
  # failed symbols are skipped and retried on the next 5-minute metrics cycle.
  def chart_candle_grids(symbols, metrics_data, since:, timeframe:, restated: false)
    ticker_by_symbol = tickers.index_by(&:base)
    grids = {}

    symbols.each_slice(CANDLE_FETCH_THREADS) do |batch|
      threads = batch.filter_map do |symbol|
        ticker = ticker_by_symbol[symbol]
        next unless ticker.present?

        Thread.new do
          result = begin
            Rails.application.executor.wrap do
              fetch_candle_series(ticker: ticker, since: since, timeframe: timeframe,
                                  restated: restated)
            end
          rescue StandardError => e
            Rails.logger.error("Candle fetch failed for #{symbol}: #{e.class}: #{e.message}")
            Result::Failure.new(e.message)
          end
          [symbol, result]
        end
      end

      ActiveSupport::Dependencies.interlock.permit_concurrent_loads do
        threads.each do |thread|
          symbol, result = thread.value
          next if result.failure? || result.data.blank?

          marks = result.data.map { |candle| [candle[0], candle[1]] } +
                  chart_live_marks(metrics_data, symbol)
          grids[symbol] = marks.sort_by(&:first)
        end
      end
    end

    grids
  end

  def calculate_pnl(from, to)
    return 0.0 if from.zero?

    (to - from).to_f / from
  end

  def initialize_metrics_data
    {
      chart: {
        labels: [],
        series: [
          [], # value
          []  # invested
        ],
        extra_series: [],
        invested_series: [], # per-asset cost basis, for the per-asset curve
        cash_series: [] # rebalance proceeds realized but not yet redeployed
      },
      total_quote_amount_invested: 0,
      total_amount_value_in_quote: 0,
      rebalance_cash: 0,
      realised_pnl: 0,
      pnl: nil,
      asset_breakdown: {},
      asset_values: {},
      num_assets: 0
    }
  end

  def initialize_totals_data
    {
      total_quote_amount_invested: 0,
      prices: [],
      amounts: []
    }
  end
end
