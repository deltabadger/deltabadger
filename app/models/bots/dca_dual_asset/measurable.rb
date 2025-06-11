module Bots::DcaDualAsset::Measurable
  extend ActiveSupport::Concern

  def metrics(force: false)
    cache_key = "bot_#{id}_metrics"
    Rails.cache.fetch(cache_key, expires_in: 30.days, force: force) do
      data = initialize_metrics_data
      transactions_array = transactions.submitted.order(created_at: :asc).pluck(:created_at,
                                                                                :price,
                                                                                :amount,
                                                                                :quote_amount,
                                                                                :base)
      return data if transactions_array.empty?

      # TODO: When transactions point to real asset ids, we can use the asset ids directly
      asset_symbol_to_id = {
        base0_asset.symbol => base0_asset_id,
        base1_asset.symbol => base1_asset_id
      }

      totals = initialize_totals_data
      transactions_array.each do |created_at, price, amount, quote_amount, base|
        next if price.zero?

        # chart data
        data[:chart][:labels] << created_at
        totals[:total_quote_amount_invested][asset_symbol_to_id[base]] += quote_amount
        totals[:total_base_amount_acquired][asset_symbol_to_id[base]] += amount
        data[:chart][:series][1] << totals[:total_quote_amount_invested].values.sum
        totals[:current_value_in_quote][asset_symbol_to_id[base]] =
          totals[:total_base_amount_acquired][asset_symbol_to_id[base]] * price
        data[:chart][:series][0] << totals[:current_value_in_quote].values.sum
        data[:chart][:extra_series][0] << totals[:total_base_amount_acquired][base0_asset_id]
        data[:chart][:extra_series][1] << totals[:total_base_amount_acquired][base1_asset_id]

        # metrics data
        totals[:prices][asset_symbol_to_id[base]] << price
        totals[:amounts][asset_symbol_to_id[base]] << amount
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
      return metrics_data if metrics_data[:chart][:labels].empty?

      result = exchange.get_tickers_prices
      return metrics_data if result.failure?

      price0 = result.data[ticker0.ticker]
      price1 = result.data[ticker1.ticker]
      return metrics_data unless price0.present? && price1.present?

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

      metrics_data
    end
  end

  def metrics_with_current_prices_and_candles(force: false)
    puts 'getting metrics with current prices and candles'
    Rails.cache.fetch(metrics_with_current_prices_and_candles_cache_key,
                      expires_in: 5.seconds, # Utilities::Time.seconds_to_end_of_five_minute_cut,
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
    metrics_data = metrics_with_current_prices_and_candles

    broadcast_replace_to(
      ["user_#{user_id}", :bot_updates],
      target: 'metrics',
      partial: 'bots/dca_dual_assets/metrics',
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

  def get_extended_chart_data_with_candles_data
    metrics_data = metrics.deep_dup
    since = metrics_data[:chart][:labels].first + 1.second
    timeframe = optimal_candles_timeframe_for_duration(Time.now.utc - since)
    expires_in = Utilities::Time.seconds_to_current_candle_close(timeframe)
    candles_cache_key = "#{ticker0.id}_candles_#{since}_#{timeframe}"
    candles0 = Rails.cache.fetch(candles_cache_key, expires_in: expires_in) do
      result = ticker0.get_candles(
        start_at: since,
        timeframe: timeframe
      )
      return result if result.failure?

      result.data[...-1]
    end

    candles_cache_key = "#{ticker1.id}_candles_#{since}_#{timeframe}"
    candles1 = Rails.cache.fetch(candles_cache_key, expires_in: expires_in) do
      result = ticker1.get_candles(
        start_at: since,
        timeframe: timeframe
      )
      return result if result.failure?

      result.data[...-1]
    end

    candles0_timestamps = candles0.map { |sublist| sublist[0] }
    candles1_timestamps = candles1.map { |sublist| sublist[0] }
    common_timestamps = candles0_timestamps & candles1_timestamps
    candles0.select! { |timestamp, _| common_timestamps.include?(timestamp) }
    candles1.select! { |timestamp, _| common_timestamps.include?(timestamp) }

    i = 0
    extended_chart_data = { labels: [], series: [[], []] }
    candles0.each_with_index do |candle, j|
      i += 1 while i < metrics_data[:chart][:labels].length - 1 && metrics_data[:chart][:labels][i + 1] <= candle[0]

      base0_amount_acquired = metrics_data[:chart][:extra_series][0][i]
      base1_amount_acquired = metrics_data[:chart][:extra_series][1][i]
      quote_amount_invested = metrics_data[:chart][:series][1][i]
      extended_chart_data[:labels] << candle[0]
      extended_chart_data[:series][0] << [
        base0_amount_acquired * candles0[j][1],
        base1_amount_acquired * candles1[j][1]
      ].sum
      extended_chart_data[:series][1] << quote_amount_invested
    end

    Result::Success.new(extended_chart_data)
  end
end
