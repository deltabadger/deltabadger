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
  METRICS_CACHE_VERSION = 'v4'.freeze

  def metrics(force: false)
    cache_key = "bot_#{id}_metrics_#{METRICS_CACHE_VERSION}"
    Rails.cache.fetch(cache_key, expires_in: 30.days, force: force) do
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
      transactions_array.each do |created_at, price, amount_exec, quote_amount_exec, amount, side, external_status|
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
        totals[:current_value_in_quote] = totals[:realized_proceeds] + (totals[:net_base] * price)
        data[:chart][:series][0] << totals[:current_value_in_quote]
        data[:chart][:extra_series][0] << totals[:net_base]
        data[:chart][:extra_series][1] << totals[:realized_proceeds]
      end

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

  def metrics_with_current_prices_cache_key
    "bot_#{id}_metrics_with_current_prices_#{METRICS_CACHE_VERSION}"
  end

  def metrics_with_current_prices_and_candles_cache_key
    "bot_#{id}_metrics_with_current_prices_and_candles_#{METRICS_CACHE_VERSION}"
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
    { ticker.base => marks.sort_by(&:first) }
  end
end
