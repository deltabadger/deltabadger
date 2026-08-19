module Bots::DcaDualAsset::Measurable
  extend ActiveSupport::Concern

  def metrics(force: false)
    cache_key = "bot_#{id}_metrics"
    Rails.cache.fetch(cache_key, expires_in: 30.days, force: force) do
      data = initialize_metrics_data
      transactions_array = transactions.submitted.order(created_at: :asc).pluck(:created_at,
                                                                                :price,
                                                                                :amount_exec,
                                                                                :quote_amount_exec,
                                                                                :amount,
                                                                                :base)
      return data if transactions_array.empty?

      # TODO: When transactions point to real asset ids, we can use the asset ids directly
      asset_symbol_to_id = {
        base0_asset.symbol => base0_asset_id,
        base1_asset.symbol => base1_asset_id
      }

      totals = initialize_totals_data
      transactions_array.each do |created_at, price, amount_exec, quote_amount_exec, amount, base|
        # Workaround for old legacy transactions in which we could not fetch the real executed amounts
        quote_amount_exec ||= price * amount
        amount_exec ||= amount
        next if price.blank? || quote_amount_exec.blank? || amount_exec.blank?
        next if quote_amount_exec.zero? || amount_exec.zero?

        # chart data
        data[:chart][:labels] << created_at
        totals[:total_quote_amount_invested][asset_symbol_to_id[base]] += quote_amount_exec
        totals[:total_base_amount_acquired][asset_symbol_to_id[base]] += amount_exec
        data[:chart][:series][1] << totals[:total_quote_amount_invested].values.sum
        totals[:current_value_in_quote][asset_symbol_to_id[base]] =
          totals[:total_base_amount_acquired][asset_symbol_to_id[base]] * price
        data[:chart][:series][0] << totals[:current_value_in_quote].values.sum
        data[:chart][:extra_series][0] << totals[:total_base_amount_acquired][base0_asset_id]
        data[:chart][:extra_series][1] << totals[:total_base_amount_acquired][base1_asset_id]

        # metrics data
        totals[:prices][asset_symbol_to_id[base]] << price
        totals[:amounts][asset_symbol_to_id[base]] << amount_exec
      end

      data[:total_base0_amount] = totals[:total_base_amount_acquired][base0_asset_id]
      data[:total_base1_amount] = totals[:total_base_amount_acquired][base1_asset_id]
      data[:base0_total_quote_amount_invested] = totals[:total_quote_amount_invested][base0_asset_id]
      data[:base1_total_quote_amount_invested] = totals[:total_quote_amount_invested][base1_asset_id]
      data[:total_quote_amount_invested] = data[:base0_total_quote_amount_invested] + data[:base1_total_quote_amount_invested]
      data[:total_base0_amount_value_in_quote] = totals[:current_value_in_quote][base0_asset_id]
      data[:total_base1_amount_value_in_quote] = totals[:current_value_in_quote][base1_asset_id]
      data[:total_amount_value_in_quote] =
        data[:total_base0_amount_value_in_quote] + data[:total_base1_amount_value_in_quote]
      data[:base0_pnl] = calculate_pnl(data[:base0_total_quote_amount_invested], data[:total_base0_amount_value_in_quote])
      data[:base1_pnl] = calculate_pnl(data[:base1_total_quote_amount_invested], data[:total_base1_amount_value_in_quote])
      data[:pnl] = calculate_pnl(data[:total_quote_amount_invested], data[:total_amount_value_in_quote])
      if totals[:amounts][base0_asset_id].sum.positive?
        data[:base0_average_buy_price] =
          Utilities::Math.weighted_average(totals[:prices][base0_asset_id],
                                           totals[:amounts][base0_asset_id])
      end
      if totals[:amounts][base1_asset_id].sum.positive?
        data[:base1_average_buy_price] =
          Utilities::Math.weighted_average(totals[:prices][base1_asset_id],
                                           totals[:amounts][base1_asset_id])
      end

      data
    end
  end

  def metrics_with_current_prices(force: false)
    Rails.cache.fetch(metrics_with_current_prices_cache_key,
                      expires_in: Utilities::Time.seconds_to_end_of_five_minute_cut,
                      force: force) do
      metrics_data = metrics.deep_dup
      return metrics_data if metrics_data[:chart][:labels].empty? || ticker0.nil? || ticker1.nil?

      result = exchange.get_tickers_prices(symbols: [ticker0.ticker, ticker1.ticker])
      return stale(metrics_data) if result.failure?

      price0 = result.data[ticker0.ticker]
      price1 = result.data[ticker1.ticker]
      return stale(metrics_data) unless price0.present? && price1.present?

      metrics_data[:total_base0_amount_value_in_quote] = metrics_data[:total_base0_amount] * price0
      metrics_data[:total_base1_amount_value_in_quote] = metrics_data[:total_base1_amount] * price1
      metrics_data[:total_amount_value_in_quote] =
        metrics_data[:total_base0_amount_value_in_quote] + metrics_data[:total_base1_amount_value_in_quote]
      metrics_data[:base0_pnl] =
        calculate_pnl(metrics_data[:base0_total_quote_amount_invested], metrics_data[:total_base0_amount_value_in_quote])
      metrics_data[:base1_pnl] =
        calculate_pnl(metrics_data[:base1_total_quote_amount_invested], metrics_data[:total_base1_amount_value_in_quote])
      metrics_data[:pnl] = calculate_pnl(metrics_data[:total_quote_amount_invested], metrics_data[:total_amount_value_in_quote])
      metrics_data[:chart][:series][0] << metrics_data[:total_amount_value_in_quote]
      metrics_data[:chart][:series][1] << metrics_data[:total_quote_amount_invested]
      metrics_data[:chart][:labels] << Time.current
      # extra_series stays parallel with labels — the chart reads holdings by index.
      metrics_data[:chart][:extra_series][0] << metrics_data[:total_base0_amount]
      metrics_data[:chart][:extra_series][1] << metrics_data[:total_base1_amount]
      # Raw per-symbol marks for the chart's price grid (see Bot::ChartSeries).
      metrics_data[:live_prices] = { ticker0.base => price0, ticker1.base => price1 }

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
        holdings: lambda { |i|
          { ticker0.base => metrics_data[:chart][:extra_series][0][i],
            ticker1.base => metrics_data[:chart][:extra_series][1][i] }
        },
        cash: ->(_i) { 0 } # dual-asset only accumulates; nothing is realized to cash
      )

      metrics_data
    end
  end

  def broadcast_metrics_update
    broadcast_replace_to(
      ["user_#{user_id}", :bot_updates],
      target: 'metrics',
      partial: 'bots/dca_dual_assets/metrics',
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
    # NOT the account total. `User#global_pnl` walks every bot the user owns, and the dashboard
    # fires one of these jobs per bot — N bots did N x N bots' worth of work, ending in two live
    # FX conversions each (17.7s warm / 158s cold for fifteen bots). The total is owned by
    # User::BroadcastGlobalPnlUpdateJob, which the page requests once and which does not depend
    # on these jobs having run.
  end

  def metrics_with_current_prices_from_cache
    Rails.cache.read(metrics_with_current_prices_cache_key)
  end

  def metrics_with_current_prices_and_candles_from_cache
    Rails.cache.read(metrics_with_current_prices_and_candles_cache_key)
  end

  private

  def metrics_with_current_prices_cache_key
    "bot_#{id}_metrics_with_current_prices"
  end

  def metrics_with_current_prices_and_candles_cache_key
    "bot_#{id}_metrics_with_current_prices_and_candles"
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
        extra_series: [
          [], # base0 amount acquired
          []  # base1 amount acquired
        ]
      },
      total_base0_amount: 0,
      total_base1_amount: 0,
      base0_total_quote_amount_invested: 0,
      base1_total_quote_amount_invested: 0,
      total_quote_amount_invested: 0,
      total_base0_amount_value_in_quote: 0,
      total_base1_amount_value_in_quote: 0,
      total_amount_value_in_quote: 0,
      base0_pnl: nil,
      base1_pnl: nil,
      pnl: nil,
      base0_average_buy_price: nil,
      base1_average_buy_price: nil
    }
  end

  def initialize_totals_data
    {
      total_quote_amount_invested: { base0_asset_id => 0, base1_asset_id => 0 },
      total_base_amount_acquired: { base0_asset_id => 0, base1_asset_id => 0 },
      current_value_in_quote: { base0_asset_id => 0, base1_asset_id => 0 },
      prices: { base0_asset_id => [], base1_asset_id => [] },
      amounts: { base0_asset_id => [], base1_asset_id => [] }
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

  # The price grid both legs are marked on: each ticker's candle opens plus its live price.
  # Only the range where BOTH have marks can be re-marked — the every-symbol coverage rule in
  # Bot::ChartSeries takes care of that, so no explicit intersection is needed here.
  def chart_price_grids(metrics_data)
    return nil if ticker0.nil? || ticker1.nil?

    since, timeframe = chart_candle_window(metrics_data[:chart])
    grids = {}
    [ticker0, ticker1].each do |ticker|
      result = fetch_candle_series(ticker: ticker, since: since, timeframe: timeframe)
      return nil if result.failure? || result.data.blank?

      marks = result.data.map { |candle| [candle[0], candle[1]] } +
              chart_live_marks(metrics_data, ticker.base)
      grids[ticker.base] = marks.sort_by(&:first)
    end
    grids
  end
end
