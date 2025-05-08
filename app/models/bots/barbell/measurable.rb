module Bots::Barbell::Measurable
  extend ActiveSupport::Concern

  def metrics(force: false)
    cache_key = "bot_#{id}_metrics"
    Rails.cache.fetch(cache_key, expires_in: 30.days, force: force) do
      data = initialize_metrics_data
      transactions_array = transactions.order(created_at: :asc).pluck(:created_at, :rate, :amount, :status, :base)
      return data if transactions_array.empty?

      # TODO: When transactions point to real asset ids, we can use the asset ids directly
      asset_symbol_to_id = {
        base0_asset.symbol => base0_asset_id,
        base1_asset.symbol => base1_asset_id
      }

      totals = initialize_totals_data
      transactions_array.each do |created_at, rate, amount, status, base|
        next if rate.nil? || rate.zero? || base.nil?

        # chart data
        data[:chart][:labels] << created_at
        amount = status == 'success' ? amount : 0
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

  def metrics_with_current_prices(metrics:, price0:, price1:)
    return metrics if metrics[:chart][:labels].empty?

    metrics[:base0_total_amount_value_in_quote] = metrics[:base0_total_amount] * price0
    metrics[:base1_total_amount_value_in_quote] = metrics[:base1_total_amount] * price1
    metrics[:total_amount_value_in_quote] =
      metrics[:base0_total_amount_value_in_quote] + metrics[:base1_total_amount_value_in_quote]
    metrics[:base0_pnl] =
      calculate_pnl(metrics[:base0_total_quote_amount_invested], metrics[:base0_total_amount_value_in_quote])
    metrics[:base1_pnl] =
      calculate_pnl(metrics[:base1_total_quote_amount_invested], metrics[:base1_total_amount_value_in_quote])
    metrics[:pnl] = calculate_pnl(metrics[:total_quote_amount_invested], metrics[:total_amount_value_in_quote])
    metrics[:chart][:series][0] << metrics[:total_amount_value_in_quote]
    metrics[:chart][:series][1] << metrics[:total_quote_amount_invested]
    metrics[:chart][:labels] << Time.current
    metrics
  end

  def get_current_prices
    result0 = get_current_price(base_asset_id: base0_asset_id, quote_asset_id: quote_asset_id)
    return result0 unless result0.success?

    result1 = get_current_price(base_asset_id: base1_asset_id, quote_asset_id: quote_asset_id)
    return result1 unless result1.success?

    Result::Success.new({ price0: result0.data, price1: result1.data })
  end

  def broadcast_metrics_update
    prices_result = get_current_prices
    metrics_data = if prices_result.success?
                     metrics_with_current_prices(metrics: metrics,
                                                 price0: prices_result.data[:price0],
                                                 price1: prices_result.data[:price1])
                   else
                     metrics
                   end

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
    prices_result = get_current_prices
    metrics_data = if prices_result.success?
                     metrics_with_current_prices(metrics: metrics,
                                                 price0: prices_result.data[:price0],
                                                 price1: prices_result.data[:price1])
                   else
                     metrics
                   end

    broadcast_replace_to(
      ["user_#{user_id}", :bot_updates],
      target: dom_id(self, :pnl),
      partial: 'bots/bot_tile/bot_tile_pnl',
      locals: { bot: self, pnl: metrics_data[:pnl] || '', loading: false }
    )
  end

  private

  def get_current_price(base_asset_id:, quote_asset_id:, force: false)
    cache_key = "exchange_#{exchange.id}_last_price_#{base_asset_id}_#{quote_asset_id}"
    price = Rails.cache.fetch(cache_key, expires_in: 5.minutes, force: force) do
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
