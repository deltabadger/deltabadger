module Bots::DcaSingleAsset::Measurable
  extend ActiveSupport::Concern

  # Bump when the cached metrics hash SHAPE or computed VALUES change, so the 30-day cache and the
  # derived caches never serve a stale result across a deploy. v2: locked-PnL (realized_proceeds +
  # net_base, extra_series gained a realized-proceeds row). v3: only confirmed (closed) executions
  # are realized — an accepted-but-unfilled (open/unknown) or cancelled order no longer counts. v4:
  # base sold beyond accumulated holdings is treated as bought at its sale price (zero-PnL excess), so
  # total_quote_amount_invested is now the TOTAL COST BASIS (real buys + phantom buys at sale price),
  # not just real cash invested — a pure liquidation reads invested ≈ proceeds and neutral
  # liquidations dilute PnL% while the $ profit comes only from the bot's real buys.
  # v5: holdings, the average buy price and the running value are restated through corporate
  # actions, so every figure in here can differ for a bot that held through one.
  METRICS_CACHE_VERSION = 'v5'.freeze

  def metrics(force: false)
    Rails.cache.fetch(metrics_cache_key, expires_in: 30.days, force: force) do
      data = initialize_metrics_data
      transactions_array = transactions.submitted.order(created_at: :asc).pluck(:created_at,
                                                                                :price,
                                                                                :amount_exec,
                                                                                :quote_amount_exec,
                                                                                :amount,
                                                                                :side,
                                                                                :external_status)
      return data if transactions_array.empty?

      totals = initialize_totals_data
      # Corporate actions are events in this walk like any fill. A pending queue rather than a
      # merge, because they are rare and this loop runs over every order the bot ever placed.
      pending_splits = split_events

      transactions_array.each do |created_at, price, amount_exec, quote_amount_exec, amount, side, external_status|
        # Before the order, not after: the restatement is a property of the position the order then
        # acts on, so a split sharing an order's timestamp is applied first.
        pending_splits = apply_due_splits(pending_splits, created_at, totals, data)

        # The "null exec == filled for the requested amount" fallback (legacy rows never backfilled
        # exec amounts) is only valid for CONFIRMED rows. An accepted-but-unfilled order (open/unknown)
        # or a cancelled one must not be assumed filled, or its requested amount would be realized
        # before any fill — those rows keep nil exec and are skipped just below.
        if external_status == 'closed'
          quote_amount_exec ||= price * amount
          amount_exec ||= amount
        end
        next if price.blank? || quote_amount_exec.blank? || amount_exec.blank?

        next if quote_amount_exec.zero? || amount_exec.zero?

        data[:chart][:labels] << created_at
        if side == 'sell'
          # Selling realizes cash and reduces holdings; the proceeds are "locked" (no longer float
          # with price). Base sold BEYOND what the bot accumulated is liquidation of externally-sourced
          # coins — treat that excess as bought at its own sale price (cost basis = sale price), so it
          # adds equally to proceeds and to invested and nets ZERO PnL, while base the bot actually
          # bought still realizes real PnL vs its buy cost.
          excess = [amount_exec - totals[:net_base], 0].max
          totals[:realized_proceeds] += quote_amount_exec
          totals[:net_base] = [totals[:net_base] - amount_exec, 0].max
          totals[:total_quote_amount_invested] += quote_amount_exec * excess / amount_exec if excess.positive?
        else
          totals[:total_quote_amount_invested] += quote_amount_exec
          totals[:net_base] += amount_exec
          # average_buy_price is over buys only
          totals[:prices] << price
          totals[:amounts] << amount_exec
        end

        data[:chart][:series][1] << totals[:total_quote_amount_invested]
        totals[:last_price] = price
        totals[:current_value_in_quote] = totals[:realized_proceeds] + (totals[:net_base] * price)
        data[:chart][:series][0] << totals[:current_value_in_quote]
        data[:chart][:extra_series][0] << totals[:net_base]
        data[:chart][:extra_series][1] << totals[:realized_proceeds]
      end

      # The ordinary case: a split lands and the bot has not traded since.
      apply_due_splits(pending_splits, nil, totals, data)

      data[:total_base_amount] = totals[:net_base]
      data[:total_quote_amount_invested] = totals[:total_quote_amount_invested]
      data[:total_realized_proceeds] = totals[:realized_proceeds]
      data[:total_amount_value_in_quote] = totals[:current_value_in_quote]
      data[:pnl] = calculate_pnl(data[:total_quote_amount_invested], data[:total_amount_value_in_quote])
      data[:average_buy_price] =
        Utilities::Math.weighted_average(totals[:prices], totals[:amounts])

      data
    end
  end

  def metrics_with_current_prices(force: false)
    Rails.cache.fetch(metrics_with_current_prices_cache_key,
                      expires_in: Utilities::Time.seconds_to_end_of_five_minute_cut,
                      force: force) do
      metrics_data = metrics.deep_dup
      return metrics_data if metrics_data[:chart][:labels].empty? || ticker.nil?

      result = exchange.get_tickers_prices(symbols: [ticker.ticker])
      # A restatement the market has not priced yet: the venue's latest trade is still the old
      # basis, and this walk's holdings are already the new one.
      return stale(metrics_data) if restated_prices_untrusted?(metrics_data)
      return stale(metrics_data) if result.failure?

      price = result.data[ticker.ticker]
      return stale(metrics_data) unless price.present?

      metrics_data[:total_amount_value_in_quote] =
        (metrics_data[:total_realized_proceeds] || 0) + (metrics_data[:total_base_amount] * price)
      metrics_data[:pnl] =
        calculate_pnl(metrics_data[:total_quote_amount_invested], metrics_data[:total_amount_value_in_quote])
      metrics_data[:chart][:series][0] << metrics_data[:total_amount_value_in_quote]
      metrics_data[:chart][:series][1] << metrics_data[:total_quote_amount_invested]
      metrics_data[:chart][:labels] << Time.current
      # extra_series stays parallel with labels: the live point holds exactly what the last
      # transaction left, and the chart reads its holdings by index (see Bot::ChartSeries).
      metrics_data[:chart][:extra_series][0] << metrics_data[:total_base_amount]
      metrics_data[:chart][:extra_series][1] << (metrics_data[:total_realized_proceeds] || 0)
      # Kept raw, per symbol: the chart needs this as the final mark on the price grid, and an
      # aggregate value cannot be inverted back into the price it was built from.
      metrics_data[:live_prices] = { ticker.base => price }

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

      metrics_data[:chart] = chart_marked_at_market(
        metrics_data[:chart], grids,
        holdings: ->(i) { { ticker.base => metrics_data[:chart][:extra_series][0][i] } },
        # Realized proceeds are locked-in cash: they do not float with the price.
        cash: ->(i) { metrics_data[:chart][:extra_series][1][i] || 0 }
      )

      metrics_data
    end
  end

  def broadcast_metrics_update
    broadcast_replace_to(
      ["user_#{user_id}", :bot_updates],
      target: 'metrics',
      partial: 'bots/dca_single_assets/metrics',
      locals: { bot: self, metrics: metrics_with_current_prices, loading: false }
    )

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

  # Folds in every restatement due at or before `until_time` (all of them when nil), and returns
  # what is left. A split multiplies the position and moves no money.
  #
  # The accumulated buy prices are divided by the same factor and the accumulated amounts
  # multiplied by it, because `average_buy_price` is a weighted average over them: left alone, a
  # split spanning two buys would average two different units and the figure would be shown against
  # a price in neither. The money paid is preserved, which is what makes the restated average the
  # cost per share the position actually has today.
  #
  # The last traded price is restated too — the running value is `proceeds + base * that price`,
  # and without it a split with no trade after it multiplies the value by the factor.
  #
  # A chart point is emitted, as a fill would: `chart_marked_at_market` interpolates between the
  # snapshots stored here, and with no snapshot at the split every candle point between it and the
  # next order would price a pre-split count. Both sides move together, so the point's value equals
  # the one before it.
  def apply_due_splits(pending, until_time, totals, data)
    # The overwhelming majority of bots have no corporate actions at all, and this runs once per
    # fill over histories that reach into the millions of them.
    return pending if pending.empty?

    due, rest = pending.partition { |at, _symbol, _factor| until_time.nil? || at <= until_time }
    due.each do |at, _symbol, factor|
      held = totals[:net_base].to_d
      # The accumulators are restated whether or not anything is held right now: they cover every
      # buy the bot has ever made, and a bot that sold out before the split and bought again after
      # it would otherwise average two different units into one price.
      totals[:net_base] = held * factor
      totals[:amounts] = totals[:amounts].map { |amount| amount.to_d * factor }
      totals[:prices] = totals[:prices].map { |price| price.to_d / factor }
      totals[:last_price] = totals[:last_price].to_d / factor if totals[:last_price].present?
      next if held.zero?

      # Only a restatement that MOVED something puts the live prices out of trust.
      data[:restated_at] = [data[:restated_at], at].compact.max

      totals[:current_value_in_quote] =
        totals[:realized_proceeds] + (totals[:net_base] * totals[:last_price].to_d)

      data[:chart][:labels] << at
      data[:chart][:series][1] << totals[:total_quote_amount_invested]
      data[:chart][:series][0] << totals[:current_value_in_quote]
      data[:chart][:extra_series][0] << totals[:net_base]
      data[:chart][:extra_series][1] << totals[:realized_proceeds]
    end
    rest
  end

  # A method, not a literal, for the same reason the composition concern has one: the caches are
  # seeded by name in tests, and dropped by name when a corporate action arrives.
  def metrics_cache_key
    "bot_#{id}_metrics_#{METRICS_CACHE_VERSION}_#{restatement_generation}"
  end

  def metrics_with_current_prices_cache_key
    "bot_#{id}_metrics_with_current_prices_#{METRICS_CACHE_VERSION}_#{restatement_generation}"
  end

  def metrics_with_current_prices_and_candles_cache_key
    "bot_#{id}_metrics_with_current_prices_and_candles_#{METRICS_CACHE_VERSION}_#{restatement_generation}"
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
          [], # value (realized_proceeds + net_base * price)
          []  # invested (cumulative buys)
        ],
        extra_series: [
          [], # net_base (buys − sells)
          []  # realized_proceeds (cash locked in by sells)
        ]
      },
      total_base_amount: 0,
      total_quote_amount_invested: 0,
      total_realized_proceeds: 0,
      total_amount_value_in_quote: 0,
      pnl: nil,
      average_buy_price: nil
    }
  end

  def initialize_totals_data
    {
      total_quote_amount_invested: 0,
      net_base: 0,
      realized_proceeds: 0,
      current_value_in_quote: 0,
      prices: [],
      amounts: []
    }
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

  # The price grid the chart is marked on: the venue's candle opens, plus the live price as a
  # final mark. nil when there are no candles — the chart then keeps its fill marks, which is
  # the only price available.
  def chart_price_grids(metrics_data)
    return nil if ticker.nil?

    since, timeframe = chart_candle_window(metrics_data[:chart])
    result = fetch_candle_series(ticker: ticker, since: since, timeframe: timeframe)
    return nil if result.failure? || result.data.blank?

    marks = result.data.map { |candle| [candle[0], candle[1]] } +
            chart_live_marks(metrics_data, ticker.base)
    chart_split_pinned_grids(ticker.base => marks.sort_by(&:first))
  end
end
