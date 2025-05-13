module Bots::Barbell::Measurable
  extend ActiveSupport::Concern

  def metrics(force: false)
    cache_key = "bot_#{id}_metrics"
    Rails.cache.fetch(cache_key, expires_in: 30.days, force: force) do
      data = initialize_metrics_data
      transactions_array = transactions.success.order(created_at: :asc).pluck(:created_at, :rate, :amount, :base)
      return data if transactions_array.empty?

      # TODO: When transactions point to real asset ids, we can use the asset ids directly
      asset_symbol_to_id = {
        base0_asset.symbol => base0_asset_id,
        base1_asset.symbol => base1_asset_id
      }

      totals = initialize_totals_data
      transactions_array.each do |created_at, rate, amount, base|
        next if rate.zero?

        # chart data
        data[:chart][:labels] << created_at
        totals[:total_quote_amount_invested][asset_symbol_to_id[base]] += amount * rate
        totals[:total_base_amount_acquired][asset_symbol_to_id[base]] += amount
        data[:chart][:series][1] << totals[:total_quote_amount_invested].values.sum
        totals[:current_value_in_quote][asset_symbol_to_id[base]] =
          totals[:total_base_amount_acquired][asset_symbol_to_id[base]] * rate
        data[:chart][:series][0] << totals[:current_value_in_quote].values.sum

        # metrics data
        totals[:rates][asset_symbol_to_id[base]] << rate
        totals[:amounts][asset_symbol_to_id[base]] << amount
      end

      data[:base0_total_amount] = totals[:total_base_amount_acquired][base0_asset_id]
      data[:base1_total_amount] = totals[:total_base_amount_acquired][base1_asset_id]
      data[:base0_total_quote_amount_invested] = totals[:total_quote_amount_invested][base0_asset_id]
      data[:base1_total_quote_amount_invested] = totals[:total_quote_amount_invested][base1_asset_id]
      data[:total_quote_amount_invested] = data[:base0_total_quote_amount_invested] + data[:base1_total_quote_amount_invested]
      data[:base0_total_amount_value_in_quote] = totals[:current_value_in_quote][base0_asset_id]
      data[:base1_total_amount_value_in_quote] = totals[:current_value_in_quote][base1_asset_id]
      data[:total_amount_value_in_quote] =
        data[:base0_total_amount_value_in_quote] + data[:base1_total_amount_value_in_quote]
      data[:base0_pnl] = calculate_pnl(data[:base0_total_quote_amount_invested], data[:base0_total_amount_value_in_quote])
      data[:base1_pnl] = calculate_pnl(data[:base1_total_quote_amount_invested], data[:base1_total_amount_value_in_quote])
      data[:pnl] = calculate_pnl(data[:total_quote_amount_invested], data[:total_amount_value_in_quote])
      data[:base0_average_buy_rate] =
        Utilities::Math.weighted_average(totals[:rates][base0_asset_id], totals[:amounts][base0_asset_id])
      data[:base1_average_buy_rate] =
        Utilities::Math.weighted_average(totals[:rates][base1_asset_id], totals[:amounts][base1_asset_id])

      data
    end
  end

  def metrics_with_current_prices(force: false)
    Rails.cache.fetch(metrics_with_current_prices_cache_key,
                      expires_in: Utilities::Time.seconds_to_next_five_minute_cut,
                      force: force) do
      return metrics if metrics[:chart][:labels].empty?

      result0 = get_last_price_from_cache(base0_asset_id, quote_asset_id)
      result1 = get_last_price_from_cache(base1_asset_id, quote_asset_id)
      return metrics unless result0.success? && result1.success?

      metrics_data = metrics.deep_dup
      metrics_data[:base0_total_amount_value_in_quote] = metrics_data[:base0_total_amount] * result0.data
      metrics_data[:base1_total_amount_value_in_quote] = metrics_data[:base1_total_amount] * result1.data
      metrics_data[:total_amount_value_in_quote] =
        metrics_data[:base0_total_amount_value_in_quote] + metrics_data[:base1_total_amount_value_in_quote]
      metrics_data[:base0_pnl] =
        calculate_pnl(metrics_data[:base0_total_quote_amount_invested], metrics_data[:base0_total_amount_value_in_quote])
      metrics_data[:base1_pnl] =
        calculate_pnl(metrics_data[:base1_total_quote_amount_invested], metrics_data[:base1_total_amount_value_in_quote])
      metrics_data[:pnl] = calculate_pnl(metrics_data[:total_quote_amount_invested], metrics_data[:total_amount_value_in_quote])
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
      partial: 'bots/metrics/metrics',
      locals: { bot: self, metrics: metrics_data, loading: false }
    )

    broadcast_replace_to(
      ["user_#{user_id}", :bot_updates],
      target: 'chart',
      partial: 'bots/chart',
      locals: { bot: self, metrics: metrics_data, loading: false }
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
      base0_total_amount: 0,
      base1_total_amount: 0,
      base0_total_quote_amount_invested: 0,
      base1_total_quote_amount_invested: 0,
      total_quote_amount_invested: 0,
      base0_total_amount_value_in_quote: 0,
      base1_total_amount_value_in_quote: 0,
      total_amount_value_in_quote: 0,
      base0_pnl: nil,
      base1_pnl: nil,
      pnl: nil,
      base0_average_buy_rate: nil,
      base1_average_buy_rate: nil
    }
  end

  def initialize_totals_data
    {
      total_quote_amount_invested: { base0_asset_id => 0, base1_asset_id => 0 },
      total_base_amount_acquired: { base0_asset_id => 0, base1_asset_id => 0 },
      current_value_in_quote: { base0_asset_id => 0, base1_asset_id => 0 },
      rates: { base0_asset_id => [], base1_asset_id => [] },
      amounts: { base0_asset_id => [], base1_asset_id => [] }
    }
  end
end
