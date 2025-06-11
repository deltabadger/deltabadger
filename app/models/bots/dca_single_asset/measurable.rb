module Bots::DcaSingleAsset::Measurable
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

      totals = initialize_totals_data
      transactions_array.each do |created_at, price, amount, quote_amount, _base|
        next if price.zero?

        # chart data
        data[:chart][:labels] << created_at
        totals[:total_quote_amount_invested] += if quote_amount.present?
                                                  quote_amount
                                                else
                                                  # TODO: Remove this once we have quote_amount in all transactions
                                                  price * amount
                                                end
        totals[:total_base_amount_acquired] += amount
        data[:chart][:series][1] << totals[:total_quote_amount_invested]
        totals[:current_value_in_quote] = totals[:total_base_amount_acquired] * price
        data[:chart][:series][0] << totals[:current_value_in_quote]
        data[:chart][:extra_series][0] << totals[:total_base_amount_acquired]

        # metrics data
        totals[:prices] << price
        totals[:amounts] << amount
      end

      data[:total_base_amount] = totals[:total_base_amount_acquired]
      data[:total_quote_amount_invested] = totals[:total_quote_amount_invested]
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
      return metrics_data if metrics_data[:chart][:labels].empty?

      result = exchange.get_tickers_prices
      return metrics_data if result.failure?

      price = result.data[ticker.ticker]
      return metrics_data unless price.present?

      metrics_data[:total_amount_value_in_quote] = metrics_data[:total_base_amount] * price
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
    metrics_data = metrics_with_current_prices_and_candles

    broadcast_replace_to(
      ["user_#{user_id}", :bot_updates],
      target: 'metrics',
      partial: 'bots/dca_single_assets/metrics',
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
          [] # amount acquired
        ]
      },
      total_base_amount: 0,
      total_quote_amount_invested: 0,
      total_amount_value_in_quote: 0,
      pnl: nil,
      average_buy_price: nil
    }
  end

  def initialize_totals_data
    {
      total_quote_amount_invested: 0,
      total_base_amount_acquired: 0,
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
    metrics_data = metrics.deep_dup
    since = metrics_data[:chart][:labels].first + 1.second
    timeframe = optimal_candles_timeframe_for_duration(Time.now.utc - since)
    candles_cache_key = "#{ticker.id}_candles_#{since}_#{timeframe}"
    expires_in = Utilities::Time.seconds_to_current_candle_close(timeframe)
    candles = Rails.cache.fetch(candles_cache_key, expires_in: expires_in) do
      result = ticker.get_candles(
        start_at: since,
        timeframe: timeframe
      )
      return result if result.failure?

      result.data[...-1]
    end

    i = 0
    extended_chart_data = { labels: [], series: [[], []] }
    candles.each do |candle|
      i += 1 while i < metrics_data[:chart][:labels].length - 1 && metrics_data[:chart][:labels][i + 1] <= candle[0]

      base_amount_acquired = metrics_data[:chart][:extra_series][0][i]
      quote_amount_invested = metrics_data[:chart][:series][1][i]
      extended_chart_data[:labels] << candle[0]
      extended_chart_data[:series][0] << base_amount_acquired * candle[1]
      extended_chart_data[:series][1] << quote_amount_invested
    end

    Result::Success.new(extended_chart_data)
  end
end
