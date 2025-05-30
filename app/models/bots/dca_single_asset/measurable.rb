module Bots::DcaSingleAsset::Measurable
  extend ActiveSupport::Concern

  def metrics(force: false)
    cache_key = "bot_#{id}_metrics"
    Rails.cache.fetch(cache_key, expires_in: 30.days, force: force) do
      data = initialize_metrics_data
      transactions_array = transactions.success.order(created_at: :asc).pluck(:created_at, :rate, :amount, :quote_amount, :base)
      return data if transactions_array.empty?

      totals = initialize_totals_data
      transactions_array.each do |created_at, rate, amount, quote_amount, _base|
        next if rate.zero?

        # chart data
        data[:chart][:labels] << created_at
        totals[:total_quote_amount_invested] += quote_amount
        totals[:total_base_amount_acquired] += amount
        data[:chart][:series][1] << totals[:total_quote_amount_invested]
        totals[:current_value_in_quote] =
          totals[:total_base_amount_acquired] * rate
        data[:chart][:series][0] << totals[:current_value_in_quote]

        # metrics data
        totals[:rates] << rate
        totals[:amounts] << amount
      end

      data[:total_base_amount] = totals[:total_base_amount_acquired]
      data[:total_quote_amount_invested] = totals[:total_quote_amount_invested]
      data[:total_amount_value_in_quote] = totals[:current_value_in_quote]
      data[:pnl] = calculate_pnl(data[:total_quote_amount_invested], data[:total_amount_value_in_quote])
      data[:average_buy_rate] =
        Utilities::Math.weighted_average(totals[:rates], totals[:amounts])

      data
    end
  end

  def metrics_with_current_prices(force: false)
    Rails.cache.fetch(metrics_with_current_prices_cache_key,
                      expires_in: Utilities::Time.seconds_to_next_five_minute_cut,
                      force: force) do
      return metrics if metrics[:chart][:labels].empty?

      result = get_last_price_from_cache(base_asset_id, quote_asset_id)
      return metrics unless result.success?

      metrics_data = metrics.deep_dup
      metrics_data[:total_amount_value_in_quote] = metrics_data[:total_base_amount] * result.data
      metrics_data[:pnl] =
        calculate_pnl(metrics_data[:total_quote_amount_invested], metrics_data[:total_amount_value_in_quote])
      metrics_data[:chart][:series][0] << metrics_data[:total_amount_value_in_quote]
      metrics_data[:chart][:series][1] << metrics_data[:total_quote_amount_invested]
      metrics_data[:chart][:labels] << Time.current

      metrics_data
    end
  end

  def broadcast_metrics_update
    metrics_data = metrics_with_current_prices

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

  private

  def metrics_with_current_prices_cache_key
    "bot_#{id}_metrics_with_current_prices"
  end

  def get_last_price_from_cache(base_asset_id, quote_asset_id)
    # we cache the price so many users can use it without hitting the API too much
    cache_key = "exchange_#{exchange.id}_last_price_for_#{base_asset_id}_#{quote_asset_id}"
    price = Rails.cache.fetch(cache_key, expires_in: Utilities::Time.seconds_to_next_five_minute_cut) do
      result = exchange.get_last_price(base_asset_id: base_asset_id, quote_asset_id: quote_asset_id)
      return result unless result.success?

      result.data
    end
    Result::Success.new(price)
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
        ]
      },
      total_base_amount: 0,
      total_quote_amount_invested: 0,
      total_amount_value_in_quote: 0,
      pnl: nil,
      average_buy_rate: nil
    }
  end

  def initialize_totals_data
    {
      total_quote_amount_invested: 0,
      total_base_amount_acquired: 0,
      current_value_in_quote: 0,
      rates: [],
      amounts: []
    }
  end
end
