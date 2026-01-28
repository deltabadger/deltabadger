module Bots::DcaIndex::Measurable
  extend ActiveSupport::Concern

  def metrics(force: false)
    cache_key = "bot_#{id}_metrics"
    Rails.cache.fetch(cache_key, expires_in: 30.days, force: force) do
      data = initialize_metrics_data
      transactions_array = transactions.submitted.order(created_at: :asc).pluck(
        :created_at,
        :price,
        :amount_exec,
        :quote_amount_exec,
        :amount,
        :base
      )
      return data if transactions_array.empty?

      totals = initialize_totals_data
      asset_totals = Hash.new { |h, k| h[k] = { amount: 0, quote_invested: 0 } }
      asset_prices = {}  # Track last known price for each asset

      transactions_array.each do |created_at, price, amount_exec, quote_amount_exec, amount, base|
        quote_amount_exec ||= price * amount
        amount_exec ||= amount
        next if price.blank? || quote_amount_exec.blank? || amount_exec.blank?
        next if quote_amount_exec.zero? || amount_exec.zero?

        # Track per-asset totals and prices
        asset_totals[base][:amount] += amount_exec
        asset_totals[base][:quote_invested] += quote_amount_exec
        asset_prices[base] = price

        # Calculate current portfolio value using last known prices
        current_value = asset_totals.sum do |symbol, asset_data|
          asset_price = asset_prices[symbol] || 0
          asset_data[:amount] * asset_price
        end

        # Chart data
        data[:chart][:labels] << created_at
        totals[:total_quote_amount_invested] += quote_amount_exec
        data[:chart][:series][0] << current_value
        data[:chart][:series][1] << totals[:total_quote_amount_invested]

        # Store snapshot of per-asset amounts for candle interpolation
        data[:chart][:extra_series] << {}.merge(asset_totals).transform_values { |v| v[:amount] }

        # Metrics data
        totals[:prices] << price
        totals[:amounts] << amount_exec
      end

      # Calculate final estimated value using last known prices
      estimated_value = asset_totals.sum do |symbol, asset_data|
        asset_price = asset_prices[symbol] || 0
        asset_data[:amount] * asset_price
      end

      data[:total_quote_amount_invested] = totals[:total_quote_amount_invested]
      data[:total_amount_value_in_quote] = estimated_value
      data[:pnl] = calculate_pnl(data[:total_quote_amount_invested], estimated_value)
      data[:asset_breakdown] = {}.merge(asset_totals)  # Create plain hash without default proc for caching
      data[:num_assets] = asset_totals.keys.size

      data
    end
  end

  def metrics_with_current_prices(force: false)
    Rails.cache.fetch(metrics_with_current_prices_cache_key,
                      expires_in: Utilities::Time.seconds_to_end_of_five_minute_cut,
                      force: force) do
      metrics_data = metrics.deep_dup
      return metrics_data if metrics_data[:chart][:labels].empty?

      result = exchange.get_tickers_prices
      return metrics_data if result.failure?

      ticker_prices = result.data
      total_value = 0

      # Calculate current value for each asset
      asset_values = {}
      metrics_data[:asset_breakdown].each do |symbol, asset_data|
        ticker = tickers.find { |t| t.base == symbol }
        next unless ticker.present?

        price = ticker_prices[ticker.ticker]
        next unless price.present?

        value = asset_data[:amount] * price
        total_value += value
        avg_price = asset_data[:amount].positive? ? asset_data[:quote_invested] / asset_data[:amount] : 0
        pnl_pct = asset_data[:quote_invested].positive? ? (value - asset_data[:quote_invested]) / asset_data[:quote_invested] : 0
        asset_values[symbol] = {
          amount: asset_data[:amount],
          quote_invested: asset_data[:quote_invested],
          current_value: value,
          current_price: price,
          avg_price: avg_price,
          pnl_percentage: pnl_pct
        }
      end

      metrics_data[:total_amount_value_in_quote] = total_value
      metrics_data[:pnl] = calculate_pnl(metrics_data[:total_quote_amount_invested], total_value)
      metrics_data[:asset_values] = asset_values
      metrics_data[:chart][:series][0] << total_value
      metrics_data[:chart][:series][1] << metrics_data[:total_quote_amount_invested]
      metrics_data[:chart][:labels] << Time.current

      metrics_data
    end
  end

  def metrics_with_current_prices_and_candles(force: false)
    Rails.cache.fetch(metrics_with_current_prices_and_candles_cache_key,
                      expires_in: Utilities::Time.seconds_to_end_of_five_minute_cut,
                      force: force) do
      metrics_with_current_prices_data = metrics_with_current_prices(force: force)
      return metrics_with_current_prices_data if metrics_with_current_prices_data[:chart][:labels].empty?

      result = get_extended_chart_data_with_candles_data
      return metrics_with_current_prices_data if result.failure?

      metrics_data = metrics_with_current_prices_data.deep_dup
      extended_chart_data = result.data
      return metrics_data if extended_chart_data[:labels].empty?

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
    metrics_data = metrics_with_current_prices_and_candles

    broadcast_replace_to(
      ["user_#{user_id}", :bot_updates],
      target: 'metrics',
      partial: 'bots/dca_indexes/metrics',
      locals: { bot: self, metrics: metrics_data, loading: false }
    )

    broadcast_replace_to(
      ["user_#{user_id}", :bot_updates],
      target: 'chart',
      partial: 'bots/chart',
      locals: { bot: self, metrics: metrics_data, loading: false, current_user: user }
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
    "bot_#{id}_metrics_with_current_prices"
  end

  def metrics_with_current_prices_and_candles_cache_key
    "bot_#{id}_metrics_with_current_prices_and_candles"
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
    return Result::Success.new(extended_chart_data) if tickers.empty?

    metrics_data = metrics.deep_dup
    return Result::Success.new(extended_chart_data) if metrics_data[:chart][:labels].empty?

    since = metrics_data[:chart][:labels].first + 1.second
    timeframe = optimal_candles_timeframe_for_duration(Time.now.utc - since)

    # Build a map of symbol -> ticker for quick lookup
    ticker_by_symbol = {}
    tickers.each do |ticker|
      ticker_by_symbol[ticker.base] = ticker
    end

    # Get unique asset symbols from transactions
    asset_symbols = metrics_data[:asset_breakdown].keys
    return Result::Success.new(extended_chart_data) if asset_symbols.empty?

    # Fetch candles for each asset's ticker
    candles_by_symbol = {}
    asset_symbols.each do |symbol|
      ticker = ticker_by_symbol[symbol]
      next unless ticker.present?

      candles_cache_key = "#{ticker.id}_candles_#{since}_#{timeframe}"
      expires_in = Utilities::Time.seconds_to_current_candle_close(timeframe)
      candles = Rails.cache.fetch(candles_cache_key, expires_in: expires_in) do
        result = ticker.get_candles(start_at: since, timeframe: timeframe)
        next nil if result.failure?

        result.data[...-1]
      end
      candles_by_symbol[symbol] = candles if candles.present?
    end

    return Result::Success.new(extended_chart_data) if candles_by_symbol.empty?

    # Use the candles from the first available ticker as the time axis
    primary_symbol = candles_by_symbol.keys.first
    primary_candles = candles_by_symbol[primary_symbol]

    i = 0
    primary_candles.each do |candle|
      candle_time = candle[0]
      i += 1 while i < metrics_data[:chart][:labels].length - 1 && metrics_data[:chart][:labels][i + 1] <= candle_time

      # Get asset amounts at this point in time
      asset_amounts = metrics_data[:chart][:extra_series][i] || {}
      quote_amount_invested = metrics_data[:chart][:series][1][i]

      # Calculate total value using candle prices for each asset
      total_value = 0
      asset_amounts.each do |symbol, amount|
        asset_candles = candles_by_symbol[symbol]
        if asset_candles.present?
          # Find the candle closest to this time
          asset_candle = asset_candles.find { |c| c[0] >= candle_time } || asset_candles.last
          price = asset_candle[1] if asset_candle # close price
          total_value += amount * price if price
        end
      end

      extended_chart_data[:labels] << candle_time
      extended_chart_data[:series][0] << total_value
      extended_chart_data[:series][1] << quote_amount_invested
    end

    Result::Success.new(extended_chart_data)
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
        extra_series: []
      },
      total_quote_amount_invested: 0,
      total_amount_value_in_quote: 0,
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
