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
      return metrics_data if result.failure?

      price = result.data[ticker.ticker]
      return metrics_data unless price.present?

      metrics_data[:total_amount_value_in_quote] =
        (metrics_data[:total_realized_proceeds] || 0) + (metrics_data[:total_base_amount] * price)
      metrics_data[:pnl] =
        calculate_pnl(metrics_data[:total_quote_amount_invested], metrics_data[:total_amount_value_in_quote])
      metrics_data[:chart][:series][0] << metrics_data[:total_amount_value_in_quote]
      metrics_data[:chart][:series][1] << metrics_data[:total_quote_amount_invested]
      metrics_data[:chart][:labels] << Time.current

      metrics_data
    end
  end

  def metrics_with_current_prices_and_candles(force: false)
    Rails.cache.fetch(metrics_with_current_prices_and_candles_cache_key,
                      expires_in: Utilities::Time.seconds_to_end_of_five_minute_cut,
                      force: force) do
      metrics_with_current_prices = metrics_with_current_prices(force: force)
      return metrics_with_current_prices if metrics_with_current_prices[:chart][:labels].empty?

      result = get_extended_chart_data_with_candles_data
      return metrics_with_current_prices if result.failure?

      metrics_data = metrics_with_current_prices.deep_dup
      extended_chart_data = result.data
      sorted_series = Utilities::Array.sort_arrays_by_first_array(
        metrics_data[:chart][:labels].concat(extended_chart_data[:labels]),
        metrics_data[:chart][:series][0].concat(extended_chart_data[:series][0]),
        metrics_data[:chart][:series][1].concat(extended_chart_data[:series][1])
      )
      metrics_data[:chart][:labels] = sorted_series[0]
      metrics_data[:chart][:series][0] = sorted_series[1]
      metrics_data[:chart][:series][1] = sorted_series[2]

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

  def broadcast_pnl_update
    metrics_data = metrics_with_current_prices

    broadcast_replace_to(
      ["user_#{user_id}", :bot_updates],
      target: dom_id(self, :pnl),
      partial: 'bots/bot_tile/bot_tile_pnl',
      locals: { bot: self, pnl: metrics_data[:pnl] || '', loading: false }
    )

    user.broadcast_global_pnl_update
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

  def get_extended_chart_data_with_candles_data
    extended_chart_data = { labels: [], series: [[], []] }
    return Result::Success.new(extended_chart_data) if ticker.nil?

    metrics_data = metrics.deep_dup
    since = metrics_data[:chart][:labels].first + 1.second
    timeframe = optimal_candles_timeframe_for_duration(Time.now.utc - since)
    result = CandleSeriesCache.fetch(ticker: ticker, since: since, timeframe: timeframe)
    return result if result.failure?

    candles = result.data

    i = 0
    candles.each do |candle|
      i += 1 while i < metrics_data[:chart][:labels].length - 1 && metrics_data[:chart][:labels][i + 1] <= candle[0]

      net_base = metrics_data[:chart][:extra_series][0][i]
      realized_proceeds = metrics_data[:chart][:extra_series][1][i]
      quote_amount_invested = metrics_data[:chart][:series][1][i]
      # candle[1] is the OPEN price — consistent with the open-time label candle[0]. Realized
      # proceeds stay fixed, so only the still-held net_base floats with the candle price.
      extended_chart_data[:labels] << candle[0]
      extended_chart_data[:series][0] << (realized_proceeds + (net_base * candle[1]))
      extended_chart_data[:series][1] << quote_amount_invested
    end

    Result::Success.new(extended_chart_data)
  end
end
